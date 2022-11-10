mod mmap;

use base::{cfg, kif, tcu};
use mmap::Mmap;

#[allow(dead_code)]
fn wait() {
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).unwrap();
    std::io::stdin().read_line(&mut s).unwrap();
}

fn main() -> Result<(), std::io::Error> {
    let tcu_mmap = Mmap::new(
        "/dev/tcu",
        tcu::MMIO_ADDR,
        tcu::MMIO_ADDR,
        2 * tcu::MMIO_SIZE,
    )?;

    // physical address needs to be the same as virtual address and it needs to be within physical memory range
    let mut msg_mmap = Mmap::new("/dev/tcumsg", 0x9000_0000, 0x9000_0000, cfg::PAGE_SIZE)?;
    println!("{:x?}", tcu_mmap);
    println!("{:x?}", msg_mmap);

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
