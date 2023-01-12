use base::io::Write;
use base::time::{TimeInstant, Runner, Instant};
use base::{
    cfg,
    errors::Error,
    io::Read,
    kif::{self, Perm},
    linux::ioctl,
    mem::MsgBuf,
    tcu::{self, EpId},
    time::{CycleInstant, Profiler},
};
use m3::tiles::Activity;
use m3::vfs::{OpenFlags, VFS, FileMode, GenericFile, FileRef};
use util::mmap::Mmap;

fn wait_for_rpl<T>(rep: EpId, rcv_buf: usize) -> Result<&'static T, Error> {
    loop {
        if let Some(off) = tcu::TCU::fetch_msg(rep) {
            let msg = tcu::TCU::offset_to_msg(rcv_buf, off);
            let rpl = msg.get_data::<kif::DefaultReply>();
            tcu::TCU::ack_msg(rep, off)?;
            return match rpl.error {
                0 => Ok(msg.get_data::<T>()),
                e => Err((e as u32).into()),
            };
        }
    }
}

fn noop_syscall(rbuf: usize) {
    let mut msg = MsgBuf::borrow_def();
    msg.set(kif::syscalls::Noop {
        opcode: kif::syscalls::Operation::NOOP.val,
    });
    tcu::TCU::send(
        tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
        &msg,
        0,
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
    )
    .unwrap();
    wait_for_rpl::<()>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, rbuf).unwrap();
}

fn bench<T: Instant, F: FnMut()>(profiler: &Profiler, name: &str, f: F) {
    let mut res = profiler.run::<T, _>(f);
    println!("\n\n{}: {:?}", name, res);
    println!("{}: {}", name, res);
    res.filter_outliers();
    println!("{} filtered: {}", name, res);
}

fn bench_custom_noop_syscall(profiler: &Profiler) {
    let (rbuf, _) = Activity::own().tile_desc().rbuf_std_space();
    bench::<CycleInstant, _>(profiler, "custom noop", || {
        noop_syscall(rbuf);
    })
}

fn bench_m3_noop_syscall(profiler: &Profiler) {
    bench::<CycleInstant, _>(profiler, "m3 noop", || {
        m3::syscalls::noop().unwrap();
    })
}

fn bench_tlb_insert(profiler: &Profiler) {
    let sample_addr = profiler as *const Profiler as usize;
    bench::<CycleInstant, _>(profiler, "tlb insert", || {
        tcu::TCU::handle_xlate_fault(sample_addr, Perm::R);
    })
}

const STR_LEN: usize = 512 * 1024;

fn bench_m3fs_read(profiler: &Profiler) {
    let mut file = VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::RW).unwrap();
    let content: String = (0..STR_LEN).map(|_| "a").collect();
    write!(file, "{}", content).unwrap();

    bench::<TimeInstant, _>(profiler, "m3fs read", || {
        let _content = file.read_to_string().unwrap();
    });

    VFS::unlink("/new-file.txt").unwrap();
}

struct WriteBenchmark {
    file: FileRef<GenericFile>,
    content: String,
}

impl WriteBenchmark {
    fn new() -> WriteBenchmark {
        WriteBenchmark {
            file: VFS::open("/bla", OpenFlags::CREATE).unwrap(),
            content: (0..STR_LEN).map(|_| "a").collect(),
        }
    }
}

impl Runner for WriteBenchmark {
    fn pre(&mut self) {
        self.file = VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::W).unwrap();
    }

    fn run(&mut self) {
        write!(self.file, "{}", self.content).unwrap();
    }

    fn post(&mut self) {
        VFS::unlink("/new-file.txt").unwrap();
    }
}

fn bench_m3fs_write(profiler: &Profiler) {
    let mut res = profiler.runner::<TimeInstant, _>(&mut WriteBenchmark::new());
    let name = "m3fs write";
    println!("\n\n{}: {:?}", name, res);
    println!("{}: {}", name, res);
    res.filter_outliers();
    println!("{} filtered: {}", name, res);
}

fn bench_m3fs_meta(profiler: &Profiler) {
    bench::<TimeInstant, _>(profiler, "m3fs meta", || {
        VFS::mkdir("/new-dir", FileMode::from_bits(0o755).unwrap()).unwrap();
        let _ = VFS::open("/new-dir/new-file", OpenFlags::CREATE).unwrap();
        VFS::link("/new-dir/new-file", "/new-link").unwrap();
        VFS::rename("/new-link", "/new-blink").unwrap();
        let _ = VFS::stat("/new-blink").unwrap();
        VFS::unlink("/new-blink").unwrap();
        VFS::unlink("/new-dir/new-file").unwrap();
        VFS::rmdir("/new-dir").unwrap();
    })
}

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;

    ioctl::register_act();
    // we can only map full pages and ENV_START is not at the beginning of a page
    let env_page_off = cfg::ENV_START & !cfg::PAGE_MASK;
    let _env_mmap = Mmap::new("/dev/mem", env_page_off, env_page_off, cfg::ENV_SIZE)?;
    let env = m3::envdata::get();

    let rbuf_phys_addr = cfg::MEM_OFFSET + 2 * cfg::PAGE_SIZE;
    let (rbuf_virt_addr, rbuf_size) = env.tile_desc().rbuf_std_space();
    let _rcv_mmap = Mmap::new("/dev/mem", rbuf_phys_addr, rbuf_virt_addr, rbuf_size)?;

    // m3 setup
    m3::env_run();

    println!("setup done");
    VFS::mount("/", "m3fs", "m3fs").unwrap();

    let profiler = Profiler::default().warmup(50).repeats(500);
    bench_custom_noop_syscall(&profiler);
    bench_m3_noop_syscall(&profiler);
    bench_tlb_insert(&profiler);
    bench_m3fs_read(&profiler);
    bench_m3fs_write(&profiler);
    bench_m3fs_meta(&profiler);

    // cleanup
    ioctl::unregister_act();

    Ok(())
}
