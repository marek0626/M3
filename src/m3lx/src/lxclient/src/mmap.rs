use std::io::{Error, ErrorKind};
use std::os::unix::io::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::{fs::OpenOptions, path::PathBuf};

#[derive(Debug)]
pub struct Mmap {
    len: usize,
    #[allow(dead_code)]
    phys: usize,
    virt: usize,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Mmap {
    pub fn new<P: AsRef<Path>>(path: P, phys: usize, virt: usize, len: usize) -> Result<Mmap, Error> {
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
                phys as libc::off_t,
            )
        };
        match base {
            libc::MAP_FAILED => Err(Error::new(ErrorKind::Other, "mmap failed")),
            x if x as usize == virt => Ok(Mmap {
                len,
                phys,
                virt,
                path: path.as_ref().into(),
            }),
            _ => Err(Error::new(
                ErrorKind::Other,
                "mmap: didn't return the right virtual address",
            )),
        }
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.virt as *mut u8
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.virt as *mut libc::c_void, self.len) };
    }
}