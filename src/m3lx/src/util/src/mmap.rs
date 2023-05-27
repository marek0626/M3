/*
 * Copyright (C) 2022-2023 Oliver Portee <oliver.portee@gmail.com>
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

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
