use crate::errors::{Code, Error};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

use std::fs::OpenOptions;

use libc;

pub fn mmap(addr: usize, size: usize) -> Result<(), Error> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/mem")
        .map_err(|_| Error::new(Code::InvArgs))?;
    let base = unsafe {
        libc::mmap(
            addr as *mut libc::c_void,
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    match base {
        libc::MAP_FAILED => {
            unsafe {
                libc::perror(0 as *const u8);
            }
            Err(Error::new(Code::InvArgs))
        },
        x if x as usize == addr => Ok(()),
        _ => Err(Error::new(Code::InvArgs)),
    }
}

pub fn munmap(addr: usize, size: usize) {
    unsafe {
        libc::munmap(addr as *mut libc::c_void, size);
    }
}
