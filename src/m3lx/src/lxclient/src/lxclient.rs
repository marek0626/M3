use base::io::Write;
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
use m3::vfs::{OpenFlags, VFS};
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

fn bench<F: FnMut()>(profiler: &Profiler, name: &str, f: F) {
    let mut res = profiler.run::<CycleInstant, _>(f);
    println!("\n\n{}: {:?}", name, res);
    println!("{}: {}", name, res);
    res.filter_outliers();
    println!("{} filtered: {}", name, res);
}

fn bench_custom_noop_syscall(profiler: &Profiler) {
    let (rbuf, _) = m3::envdata::get().tile_desc().rbuf_std_space();
    bench(profiler, "custom noop", || {
        noop_syscall(rbuf);
    })
}

fn bench_m3_noop_syscall(profiler: &Profiler) {
    bench(profiler, "m3 noop", || {
        m3::syscalls::noop().unwrap();
    })
}

fn bench_tlb_insert(profiler: &Profiler) {
    let sample_addr = profiler as *const Profiler as usize;
    bench(profiler, "tlb insert", || {
        tcu::TCU::handle_xlate_fault(sample_addr, Perm::R);
    })
}

fn bench_m3fs(profiler: &Profiler) {
    let new_file_contents = "test\ntest";
    bench(profiler, "m3fs meta", || {
        {
            let mut file = VFS::open("/new-file.txt", OpenFlags::W | OpenFlags::CREATE).unwrap();
            write!(file, "{}", new_file_contents).unwrap();
        }
        {
            let mut file = VFS::open("/new-file.txt", OpenFlags::R).unwrap();
            let contents = file.read_to_string().unwrap();
            assert!(contents == new_file_contents);
        }
        {
            VFS::unlink("/new-file.txt").unwrap();
        }
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
    println!("{:#?}", env);

    let rbuf_phys_addr = cfg::MEM_OFFSET + 2 * cfg::PAGE_SIZE;
    let (rbuf_virt_addr, rbuf_size) = env.tile_desc().rbuf_std_space();
    let _rcv_mmap = Mmap::new("/dev/mem", rbuf_phys_addr, rbuf_virt_addr, rbuf_size)?;

    // m3 setup
    m3::env_run();

    println!("setup done");
    VFS::mount("/", "m3fs", "m3fs").unwrap();

    let profiler = Profiler::default().warmup(50).repeats(1000);
    bench_custom_noop_syscall(&profiler);
    bench_m3_noop_syscall(&profiler);
    bench_tlb_insert(&profiler);
    bench_m3fs(&profiler);

    // cleanup
    ioctl::unregister_act();

    Ok(())
}
