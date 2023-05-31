/*
 * Copyright (C) 2023 Nils Asmussen, Barkhausen Institut
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

use base::env;
use base::linux::{self, ioctl};

fn main() -> Result<(), std::io::Error> {
    linux::init();

    ioctl::register_act();

    if unsafe { libc::fork() } == 0 {
        let name = env::args().next().unwrap();
        let mut bytes = [0u8; 1024];
        let mut argv: Vec<*const u8> = vec![];
        let mut pos = 0;
        for arg in env::args() {
            // store null-terminated argument into bytes array
            bytes[pos..pos + arg.len()].copy_from_slice(arg.as_bytes());
            bytes[pos + arg.len()] = b'\0';
            // store pointer in argv array
            unsafe {
                argv.push(bytes.as_ptr().add(pos));
            }
            pos += arg.len() + 1;
        }
        argv.push(std::ptr::null());

        println!(
            "Running {} {:?} in process {}",
            name,
            env::args().skip(1).collect::<Vec<_>>(),
            unsafe { libc::getpid() },
        );

        let res = unsafe { libc::execvp(name.as_ptr(), argv.as_ptr()) };
        if res != 0 {
            println!("execvp failed: {}", res);
        }
    }
    else {
        let mut status = 0;
        let pid = unsafe { libc::wait(&mut status as *mut _) };
        println!("Process {} exited with status {}", pid, status);
    }

    ioctl::unregister_act();

    Ok(())
}
