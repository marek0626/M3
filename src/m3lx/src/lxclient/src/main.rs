mod mmap;

use base::{
    cfg,
    errors::Error,
    kif, tcu,
    time::{CycleInstant, Profiler},
};
use mmap::Mmap;
use std::os::unix::prelude::AsRawFd;

// this is defined in linux/drivers/tcu.cc (and the right value will be printed on driver initialization during boot time)
const IOCTL_XLATE_FAULT: u64 = 0x40087101;

const MSG_BUF_ADDR: usize = 0x4000_0000;
const RCV_BUF_ADDR: usize = cfg::TILEMUX_RBUF_SPACE;

pub const MAX_MSG_SIZE: usize = 512;

#[repr(C)]
struct IoctlXlateFaultArg {
    virt: u64,
    phys: u64,
    perm: u32,
    asid: u16,
}

fn tlb_insert_addr(addr: usize, perm: kif::Perm, asid: u16) {
    let arg = IoctlXlateFaultArg {
        virt: addr as u64,
        phys: addr as u64,
        perm: perm.bits(),
        asid,
    };
    let tcu_dev = std::fs::File::open("/dev/tcu").unwrap();
    unsafe {
        let res = libc::ioctl(
            tcu_dev.as_raw_fd(),
            IOCTL_XLATE_FAULT,
            &arg as *const IoctlXlateFaultArg,
        );
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl call to insert tlb entry failed");
        }
    }
}

fn send_msg<T>(msg_obj: T) -> Result<(), Error> {
    let size = std::mem::size_of_val(&msg_obj);
    // let algn = std::mem::align_of_val(&msg_obj);
    // assert!(size <= MAX_MSG_SIZE);
    // assert!(algn <= cfg::PAGE_SIZE);
    unsafe { (MSG_BUF_ADDR as *mut T).write(msg_obj) };
    tcu::TCU::send_aligned(
        tcu::KPEX_SEP,
        MSG_BUF_ADDR as *const u8,
        size,
        0,
        tcu::KPEX_REP,
    )
}

fn wait_for_rpl() -> Result<(), Error> {
    loop {
        if let Some(off) = tcu::TCU::fetch_msg(tcu::KPEX_REP) {
            let msg = tcu::TCU::offset_to_msg(RCV_BUF_ADDR, off);
            let rpl = msg.get_data::<kif::DefaultReply>();
            tcu::TCU::ack_msg(tcu::KPEX_REP, off)?;
            return match rpl.error {
                0 => Ok(()),
                e => Err((e as u32).into()),
            };
        }
    }
}

fn main() -> Result<(), std::io::Error> {
    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;
    let _msg_mmap = Mmap::new("/dev/mem", MSG_BUF_ADDR, MSG_BUF_ADDR, cfg::PAGE_SIZE)?;
    let _rcv_mmap = Mmap::new("/dev/mem", RCV_BUF_ADDR, RCV_BUF_ADDR, cfg::PAGE_SIZE)?;

    tlb_insert_addr(MSG_BUF_ADDR, kif::Perm::R, 0xffff);

    let msg = kif::tilemux::Noop {
        op: kif::tilemux::Calls::NOOP.val,
    };

    let mut profiler = Profiler::default().warmup(50).repeats(1000);
    let mut res = profiler.run::<CycleInstant, _>(|| {
        send_msg(msg).unwrap();
        wait_for_rpl().unwrap();
    });
    println!("{}", res);
    res.filter_outliers();
    println!("filtered outliers: {}", res);

    Ok(())
}
