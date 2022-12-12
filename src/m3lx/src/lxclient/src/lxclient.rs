use base::{
    cfg,
    // errors::Error,
    // kif,
    linux::ioctl,
    // mem::MsgBuf,
    tcu, time::{Profiler, CycleInstant},
    // time::Runner,
};
use util::mmap::Mmap;

// TODO: use the right value here
// const US_RCV_BUF_ADDR: usize = cfg::TILEMUX_RBUF_SPACE + cfg::PAGE_SIZE;

pub const MAX_MSG_SIZE: usize = 512;

// #[inline(always)]
// fn wait_for_rpl<T>(rep: EpId, rcv_buf: usize) -> Result<&'static T, Error> {
//     loop {
//         if let Some(off) = tcu::TCU::fetch_msg(rep) {
//             let msg = tcu::TCU::offset_to_msg(rcv_buf, off);
//             let rpl = msg.get_data::<kif::DefaultReply>();
//             tcu::TCU::ack_msg(rep, off)?;
//             return match rpl.error {
//                 0 => Ok(msg.get_data::<T>()),
//                 e => Err((e as u32).into()),
//             };
//         }
//     }
// }
// 
// struct Tester;
// 
// impl Runner for Tester {
//     fn pre(&mut self) {
//         let mut msg = MsgBuf::new();
//         msg.set(kif::syscalls::Noop {
//             opcode: kif::syscalls::Operation::NOOP.val,
//         });
//         tcu::TCU::send(
//             tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
//             &msg,
//             0,
//             tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
//         )
//         .unwrap();
//     }
// 
//     fn run(&mut self) {
//         wait_for_rpl::<()>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR).unwrap();
//     }
// }
// 
// #[inline(never)]
// fn noop_syscall() {
//     let mut msg = MsgBuf::new();
//     msg.set(kif::syscalls::Noop {
//         opcode: kif::syscalls::Operation::NOOP.val,
//     });
//     tcu::TCU::send(
//         tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
//         &msg,
//         0,
//         tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
//     )
//     .unwrap();
//     wait_for_rpl::<()>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR).unwrap();
// }
// 
// #[allow(unused)]
// fn bench_noop_syscall() {
//     use base::time::{CycleInstant, Profiler};
// 
//     let profiler = Profiler::default().warmup(50).repeats(1000);
//     let mut res = profiler.run::<CycleInstant, _>(|| {
//         noop_syscall();
//     });
//     // let mut res = profiler.runner::<CycleInstant, _>(&mut Tester);
//     println!("{}", res);
//     res.filter_outliers();
//     println!("{}", res);
// }

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;

    ioctl::register_act();
    // we can only map full pages and ENV_START is not at the beginning of a page
    let env_page_off = cfg::ENV_START & !cfg::PAGE_MASK;
    let _env_mmap = Mmap::new("/dev/mem", env_page_off, env_page_off, cfg::ENV_SIZE)?;
    let env = m3::envdata::get();
    let (addr, size) = env.tile_desc().rbuf_std_space();
    println!("user std rcv buf addr: {:#x}, size: {:#x}", addr, size);
    let _rcv_mmap = Mmap::new("/dev/mem", addr, addr, size)?;

    // m3 setup
    m3::env_run();

    println!("setup done");

    let profiler = Profiler::default().warmup(50).repeats(1000);
    let mut res = profiler.run::<CycleInstant, _>(|| {
        m3::syscalls::noop().unwrap();
    });
    println!("{}", res);
    res.filter_outliers();
    println!("{}", res);

    // cleanup
    ioctl::unregister_act();

    Ok(())
}
