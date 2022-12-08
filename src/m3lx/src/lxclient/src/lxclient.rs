use base::{
    cfg,
    errors::Error,
    kif,
    linux::ioctl,
    mem::MsgBuf,
    tcu::{self, EpId},
    time::Runner,
};
use util::mmap::Mmap;

// TODO: use the right value here
const US_RCV_BUF_ADDR: usize = cfg::TILEMUX_RBUF_SPACE + cfg::PAGE_SIZE;

pub const MAX_MSG_SIZE: usize = 512;

// #[inline(always)]
// fn send_msg<T>(msg_obj: T, sep: EpId, rep: EpId) -> Result<(), Error> {
//     let size = std::mem::size_of_val(&msg_obj);
//     // let algn = std::mem::align_of_val(&msg_obj);
//     // assert!(size <= MAX_MSG_SIZE);
//     // assert!(algn <= cfg::PAGE_SIZE);
//     unsafe { (MSG_BUF_ADDR as *mut T).write(msg_obj) };
//     tcu::TCU::send_aligned(sep, MSG_BUF_ADDR as *const u8, size, 0, rep)
// }

#[inline(always)]
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

struct Tester;

impl Runner for Tester {
    fn pre(&mut self) {
        let mut msg = MsgBuf::new();
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
    }

    fn run(&mut self) {
        wait_for_rpl::<()>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR).unwrap();
    }
}

#[inline(never)]
fn noop_syscall() {
    let mut msg = MsgBuf::new();
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
    wait_for_rpl::<()>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR).unwrap();
}

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;
    let _us_rcv_mmap = Mmap::new("/dev/mem", US_RCV_BUF_ADDR, US_RCV_BUF_ADDR, cfg::PAGE_SIZE)?;

    // we can only map full pages and ENV_START is not at the beginning of a page
    // let env_page_off = cfg::ENV_START & !cfg::PAGE_MASK;
    // let _env_mmap = Mmap::new("/dev/mem", env_page_off, env_page_off, cfg::ENV_SIZE)?;
    // let env = base::envdata::get();
    // let actid = env.act_id as u16;

    ioctl::register_act();

    println!("setup done");
    // println!("{:#?}", env);

    use base::time::{CycleInstant, Profiler};

    let profiler = Profiler::default().warmup(50).repeats(1000);
    let mut res = profiler.run::<CycleInstant, _>(|| {
        noop_syscall();
    });
    // let mut res = profiler.runner::<CycleInstant, _>(&mut Tester);
    println!("{}", res);
    res.filter_outliers();
    println!("{}", res);
    noop_syscall();

    // cleanup
    ioctl::unregister_act();

    Ok(())
}
