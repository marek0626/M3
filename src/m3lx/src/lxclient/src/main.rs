mod mmap;

use std::os::unix::prelude::AsRawFd;

use base::{cfg, kif, tcu};
use mmap::Mmap;

// this is defined in linux/drivers/tcu.cc (and the right value will be printed on driver initialization during boot time)
const IOCTL_XLATE_FAULT: u64 = 0x40087101;

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

fn main() -> Result<(), std::io::Error> {
    let tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, tcu::MMIO_ADDR, tcu::MMIO_SIZE)?;

    // physical address needs to be the same as virtual address and it needs to be within physical memory range
    let msg_addr = 0x9000_0000usize;
    let mut msg_mmap = Mmap::new("/dev/mem", msg_addr, msg_addr, cfg::PAGE_SIZE)?;
    println!("{:x?}", tcu_mmap);
    println!("{:x?}", msg_mmap);

    // TODO: What is the asid?
    tlb_insert_addr(msg_addr, kif::Perm::R, 0xffff);

    let msg = kif::tilemux::Exit {
        op: 0,
        act_sel: 1,
        code: 2,
    };

    // TODO: assert alignment
    let len = std::mem::size_of_val(&msg);
    assert!(msg_mmap.len() >= len);
    let msg_base = msg_mmap.as_mut_ptr();
    unsafe { (msg_base as *mut kif::tilemux::Exit).write(msg) };
    tcu::TCU::send_aligned(tcu::KPEX_SEP, msg_base, len, 0, tcu::KPEX_REP)
        .expect("TCU::send failed");
    Ok(())
}
