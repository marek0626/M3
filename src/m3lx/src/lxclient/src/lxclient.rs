use base::{
    cfg,
    errors::Error,
    kif,
    tcu::{self, ActId, EpId},
    time::Runner,
    linux::ioctl,
};
use util::mmap::Mmap;

const MSG_BUF_ADDR: usize = 0x4000_0000;
const TM_RCV_BUF_ADDR: usize = cfg::TILEMUX_RBUF_SPACE;
const US_RCV_BUF_ADDR: usize = TM_RCV_BUF_ADDR + cfg::PAGE_SIZE;

pub const MAX_MSG_SIZE: usize = 512;


#[inline(always)]
fn send_msg<T>(msg_obj: T, sep: EpId, rep: EpId) -> Result<(), Error> {
    let size = std::mem::size_of_val(&msg_obj);
    // let algn = std::mem::align_of_val(&msg_obj);
    // assert!(size <= MAX_MSG_SIZE);
    // assert!(algn <= cfg::PAGE_SIZE);
    unsafe { (MSG_BUF_ADDR as *mut T).write(msg_obj) };
    tcu::TCU::send_aligned(sep, MSG_BUF_ADDR as *const u8, size, 0, rep)
}

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

// send and receive LxAct sidecall
fn send_receive_lx_act() {
    let msg = kif::tilemux::LxAct {
        op: kif::tilemux::Calls::LX_ACT.val,
    };
    send_msg(msg, tcu::KPEX_SEP, tcu::KPEX_REP).unwrap();
    wait_for_rpl::<()>(tcu::KPEX_REP, TM_RCV_BUF_ADDR).unwrap();
}

// send and receive Exit sidecall
fn send_receive_exit(id: ActId) {
    let msg = kif::tilemux::Exit {
        op: kif::tilemux::Calls::EXIT.val,
        act_sel: id as u64,
        code: 0,
    };
    send_msg(msg, tcu::KPEX_SEP, tcu::KPEX_REP).unwrap();
    wait_for_rpl::<kif::DefaultReply>(tcu::KPEX_REP, TM_RCV_BUF_ADDR).unwrap();
}

struct Tester;

impl Runner for Tester {
    fn pre(&mut self) {
        let noop = kif::syscalls::Noop {
            opcode: kif::syscalls::Operation::NOOP.val,
        };
        send_msg(
            noop,
            tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
            tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
        )
        .unwrap();
    }

    fn run(&mut self) {
        wait_for_rpl::<kif::DefaultReply>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR)
            .unwrap();
    }
}

#[inline(never)]
fn noop_syscall() {
    let noop = kif::syscalls::Noop {
        opcode: kif::syscalls::Operation::NOOP.val,
    };
    send_msg(
        noop,
        tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
    )
    .unwrap();
    wait_for_rpl::<kif::DefaultReply>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR)
        .unwrap();
}

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;
    let _msg_mmap = Mmap::new("/dev/mem", MSG_BUF_ADDR, MSG_BUF_ADDR, cfg::PAGE_SIZE)?;
    let _tm_rcv_mmap = Mmap::new("/dev/mem", TM_RCV_BUF_ADDR, TM_RCV_BUF_ADDR, cfg::PAGE_SIZE)?;
    let _us_rcv_mmap = Mmap::new("/dev/mem", US_RCV_BUF_ADDR, US_RCV_BUF_ADDR, cfg::PAGE_SIZE)?;

    send_receive_lx_act();

    // we can only map full pages and ENV_START is not at the beginning of a page
    let env_page_off = cfg::ENV_START & !cfg::PAGE_MASK;
    let _env_mmap = Mmap::new("/dev/mem", env_page_off, env_page_off, cfg::ENV_SIZE)?;
    let env = base::envdata::get();
    let actid = env.act_id as u16;

    ioctl::register_act(actid);
    ioctl::switch_to_user_mode();

    println!("setup done.");
    println!("{:#?}", env);

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
    ioctl::switch_to_tm_mode();
    send_receive_exit(actid);
    ioctl::unregister_act();

    Ok(())
}
