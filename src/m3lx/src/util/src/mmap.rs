use std::fs::OpenOptions;
use std::io::{Error, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use num_enum::IntoPrimitive;

#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
#[repr(usize)]
pub enum MemType {
    TCU,
    Environment,
    StdRecvBuf,
}

#[derive(Debug)]
pub struct Mmap {
    len: usize,
    virt: usize,
}

impl Mmap {
    pub fn new<P: AsRef<Path>>(
        path: P,
        virt: usize,
        ty: MemType,
        len: usize,
    ) -> Result<Mmap, Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_SYNC)
            .open(&path)?;
        let base = unsafe {
            libc::mmap(
                virt as *mut libc::c_void,
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED | libc::MAP_SYNC,
                file.as_raw_fd(),
                (ty as libc::off_t) << 12,
            )
        };
        match base {
            libc::MAP_FAILED => {
                unsafe {
                    libc::perror(0 as *const u8);
                }
                Err(Error::new(ErrorKind::Other, "mmap failed"))
            },
            x if x as usize == virt => Ok(Mmap { len, virt }),
            _ => Err(Error::new(
                ErrorKind::Other,
                "mmap: didn't return the right virtual address",
            )),
        }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.virt as *mut libc::c_void, self.len) };
    }
}
