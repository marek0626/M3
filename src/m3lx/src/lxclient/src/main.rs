mod mmap;

use base::{
    cfg,
    errors::Error,
    kif,
    tcu::{self, ActId, EpId},
};
use mmap::Mmap;
use std::os::unix::prelude::AsRawFd;

// this is defined in linux/drivers/tcu.cc (and the right value will be printed on driver initialization during boot time)
const IOCTL_RGSTR_ACT: u64 = 0x40087101;
// const IOCTL_TO_TMX_MD: u64 = 0x7102;
const IOCTL_TO_USR_MD: u64 = 0x7103;
const IOCTL_TLB_INSRT: u64 = 0x40087104;

const MSG_BUF_ADDR: usize = 0x4000_0000;
const TM_RCV_BUF_ADDR: usize = cfg::TILEMUX_RBUF_SPACE;
const US_RCV_BUF_ADDR: usize = TM_RCV_BUF_ADDR + cfg::PAGE_SIZE;

pub const MAX_MSG_SIZE: usize = 512;

#[repr(C)]
struct TlbInsert {
    phys: u64,
    virt: u32,
}

// wrapper around ioctl call
fn tlb_insert_addr(virt: usize, phys: usize) {
    assert!(virt >> 32 == 0);
    let arg = TlbInsert {
        phys: phys as u64,
        virt: virt as u32,
    };
    let tcu_dev = std::fs::File::open("/dev/tcu").unwrap();
    unsafe {
        let res = libc::ioctl(tcu_dev.as_raw_fd(), IOCTL_TLB_INSRT, &arg as *const _);
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl call for inserting tlb entry failed");
        }
    }
}

// wrapper around ioctl call
fn register_act(actid: ActId) {
    let tcu_dev = std::fs::File::open("/dev/tcu").unwrap();
    unsafe {
        let res = libc::ioctl(tcu_dev.as_raw_fd(), IOCTL_RGSTR_ACT, &actid as *const _);
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl call to register activity failed");
        }
    }
}

fn switch_to_user_mode() {
    let tcu_dev = std::fs::File::open("/dev/tcu").unwrap();
    unsafe {
        let res = libc::ioctl(tcu_dev.as_raw_fd(), IOCTL_TO_USR_MD);
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl for switching to user mode failed");
        }
    }
}

fn send_msg<T>(msg_obj: T, sep: EpId, rep: EpId) -> Result<(), Error> {
    let size = std::mem::size_of_val(&msg_obj);
    // let algn = std::mem::align_of_val(&msg_obj);
    // assert!(size <= MAX_MSG_SIZE);
    // assert!(algn <= cfg::PAGE_SIZE);
    unsafe { (MSG_BUF_ADDR as *mut T).write(msg_obj) };
    tcu::TCU::send_aligned(sep, MSG_BUF_ADDR as *const u8, size, 0, rep)
}

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

// send and receiver LxAct sidecall
fn send_receive_lx_act() -> ActId {
    let msg = kif::tilemux::LxAct {
        op: kif::tilemux::Calls::LX_ACT.val,
    };
    send_msg(msg, tcu::KPEX_SEP, tcu::KPEX_REP).unwrap();
    let rpl = wait_for_rpl::<kif::tilemux::LxActReply>(tcu::KPEX_REP, TM_RCV_BUF_ADDR).unwrap();
    rpl.actid as ActId
}

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;
    let _msg_mmap = Mmap::new("/dev/mem", MSG_BUF_ADDR, MSG_BUF_ADDR, cfg::PAGE_SIZE)?;
    let _tm_rcv_mmap = Mmap::new("/dev/mem", TM_RCV_BUF_ADDR, TM_RCV_BUF_ADDR, cfg::PAGE_SIZE)?;
    let _us_rcv_mmap = Mmap::new("/dev/mem", US_RCV_BUF_ADDR, US_RCV_BUF_ADDR, cfg::PAGE_SIZE)?;

    // insert tlb for tm for msg buf
    tlb_insert_addr(MSG_BUF_ADDR, MSG_BUF_ADDR);
    // send ActLx sidecall
    let actid = send_receive_lx_act();
    println!("actid: {}", actid);
    // register act in linux kernel
    register_act(actid);
    // switch to user mode
    switch_to_user_mode();
    // insert tlb for user for msg buf
    tlb_insert_addr(MSG_BUF_ADDR, MSG_BUF_ADDR);

    println!("setup done");

    let noop = kif::syscalls::Noop {
        opcode: kif::syscalls::Operation::NOOP.val,
    };
    send_msg(
        noop,
        tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
    )
    .unwrap();
    // TODO: now, we cast to DefaultReply twice :/
    wait_for_rpl::<kif::DefaultReply>(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, US_RCV_BUF_ADDR).unwrap();

    Ok(())
}
