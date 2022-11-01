#[macro_use]
mod int_enum;
mod sidecall_interface;
mod tcu;

use libc;
use sidecall_interface::*;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use tcu::*;

fn write(addr: *mut Reg, reg_index: usize, data: Reg) {
    unsafe {
        let addr = addr.offset(reg_index as isize);
        *addr = data;
    }
}

fn main() {
    println!("Hello, World!");
    let path = Path::new("/dev/mem");
    let file = match File::open(&path) {
        Err(e) => panic!("couldn't open {}: {}", path.display(), e),
        Ok(file) => file,
    };

    let tcu_base = unsafe {
        libc::mmap(
            0 as *mut libc::c_void,
            MMIO_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            MMIO_ADDR as libc::off_t,
        ) as *mut Reg
    };

    let msg = Exit {
        op: 0,
        act_sel: 12297829382473034410,
        code: 12297829382473034410,
    };
    let len = std::mem::size_of::<Exit>();
    let msg: &[u8] = unsafe { std::slice::from_raw_parts(&msg as *const Exit as *const u8, len) };
    let mut msg_buf: [u8; 512] = [0; 512];
    msg_buf[..len].copy_from_slice(msg);

    let addr = &msg_buf as *const u8 as u64;
    println!("address of message: {:#x}", addr);
    assert!(addr < (1 << 32));
    let data = ((len as u64) << 32) | addr;

    write(
        tcu_base,
        UNPRIV_REGS_START + UnprivReg::DATA.val as usize,
        data,
    );
    let reply_ep = KPEX_REP as Reg;
    let ep = KPEX_SEP as Reg;
    let cmd: Reg = (reply_ep << 25) | (ep << 4) | (CmdOpCode::SEND.val as Reg);
    write(
        tcu_base,
        UNPRIV_REGS_START + UnprivReg::COMMAND.val as usize,
        cmd,
    );
}
