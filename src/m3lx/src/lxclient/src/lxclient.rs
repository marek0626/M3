use util::mmap::Mmap;

use m3::{
    cfg,
    errors::Error,
    io::{Read, Write},
    kif::{self, Perm},
    linux::ioctl,
    mem::MsgBuf,
    tcu::{self, EpId},
    tiles::Activity,
    time::{
        CycleDuration, CycleInstant, Duration, Profiler, Results, Runner, TimeDuration, TimeInstant,
    },
    vfs::{FileMode, FileRef, GenericFile, OpenFlags, VFS},
};

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

#[inline(never)]
fn bench_custom_noop_syscall(profiler: &Profiler) -> Results<CycleDuration> {
    let (rbuf, _) = Activity::own().tile_desc().rbuf_std_space();
    profiler.run::<CycleInstant, _>(|| {
        noop_syscall(rbuf);
    })
}

#[inline(never)]
fn bench_m3_noop_syscall(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        m3::syscalls::noop().unwrap();
    })
}

#[inline(never)]
fn bench_tlb_insert(profiler: &Profiler) -> Results<CycleDuration> {
    let sample_addr = profiler as *const Profiler as usize;
    profiler.run::<CycleInstant, _>(|| {
        tcu::TCU::handle_xlate_fault(sample_addr, Perm::R);
    })
}

#[inline(never)]
fn bench_os_call(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        ioctl::noop();
    })
}

#[inline(never)]
fn bench_os_call_arg(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        ioctl::noop_arg(0, 0);
    })
}

const STR_LEN: usize = 512 * 1024;

#[inline(never)]
fn bench_m3fs_read(profiler: &Profiler) -> Results<TimeDuration> {
    let mut file = VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::RW).unwrap();
    let content: String = (0..STR_LEN).map(|_| "a").collect();
    write!(file, "{}", content).unwrap();

    let res = profiler.run::<TimeInstant, _>(|| {
        let _content = file.read_to_string().unwrap();
    });

    VFS::unlink("/new-file.txt").unwrap();
    res
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

#[inline(never)]
fn bench_m3fs_write(profiler: &Profiler) -> Results<TimeDuration> {
    profiler.runner::<TimeInstant, _>(&mut WriteBenchmark::new())
}

#[inline(never)]
fn bench_m3fs_meta(profiler: &Profiler) -> Results<TimeDuration> {
    profiler.run::<TimeInstant, _>(|| {
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

fn print_csv(data: Vec<(String, Vec<u64>)>) {
    if data.is_empty() {
        return;
    }
    let header = data
        .iter()
        .map(|column| format!("\"{}\"", column.0))
        .collect::<Vec<String>>()
        .join(",");
    println!("{}", header);
    let n_row = data[0].1.len();
    for r in 0..n_row {
        let row = data
            .iter()
            .map(|(_, d)| d[r].to_string())
            .collect::<Vec<String>>()
            .join(",");
        println!("{}", row);
    }
}

fn print_summary<T: Duration>(name: &str, res: &mut Results<T>) {
    println!("\n\n{}:", name);
    println!("{}", res);
    res.filter_outliers();
    println!("filtered: {}", res);
}

fn _column<T: Duration>(name: &str, res: &Results<T>) -> (String, Vec<u64>) {
    (name.into(), res.times.iter().map(|t| t.as_raw()).collect())
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

    println!("setup done\n");

    VFS::mount("/", "m3fs", "m3fs").unwrap();
    let profiler = Profiler::default().warmup(10).repeats(100);

    let mut oscall = bench_os_call(&profiler);
    let mut oscall_arg = bench_os_call_arg(&profiler);
    let mut cnoop = bench_custom_noop_syscall(&profiler);
    let mut m3noop = bench_m3_noop_syscall(&profiler);
    let mut tlb = bench_tlb_insert(&profiler);
    let mut read = bench_m3fs_read(&profiler);
    let mut write = bench_m3fs_write(&profiler);
    let mut meta = bench_m3fs_meta(&profiler);

    print_csv(vec![
        _column("custom noop", &cnoop),
        _column("m3 noop", &m3noop),
        _column("oscall arg", &oscall_arg),
        _column("tlb insert", &tlb),
        _column("m3fs read", &read),
        _column("m3fs write", &write),
        _column("m3fs meta", &meta),
    ]);

    print_summary("custom noop", &mut cnoop);
    print_summary("m3 noop", &mut m3noop);
    print_summary("oscall", &mut oscall);
    print_summary("oscall arg", &mut oscall_arg);
    print_summary("tlb insert", &mut tlb);
    print_summary("m3fs read", &mut read);
    print_summary("m3fs write", &mut write);
    print_summary("m3fs meta", &mut meta);
    // cleanup
    ioctl::unregister_act();

    Ok(())
}
