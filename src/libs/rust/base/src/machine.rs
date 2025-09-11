/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
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

//! Machine-specific functions

use crate::cfg;
use crate::env;
use crate::tcu;

extern "C" {
    pub fn gem5_writefile(src: *const u8, len: u64, offset: u64, file: u64);
    pub fn gem5_shutdown(delay: u64);
}

pub fn write(buf: &[u8]) -> usize {
    let amount = tcu::TCU::print(buf);
    #[cfg(all(M3_LX = "1", M3_TARGET = "gem5"))]
    unsafe {
        libc::write(1, buf.as_ptr() as *const libc::c_void, buf.len())
    };
    #[cfg(not(M3_LX = "1"))]
    {
        use crate::cpu::{CPUOps, CPU};
        if env::boot().platform == env::Platform::Gem5 {
            unsafe {
                let file = b"stdout\0";
                // make sure the buffer is actually written before we call gem5_writefile
                // without this it might end up in the store buffer, where gem5 doesn't see it.
                // note that the fence is only effective together with the volatile reads below
                // because it just controls ordering of memory accesses and not instructions.
                CPU::memory_barrier();
                // touch the string first to cause a page fault, if required. gem5 assumes that it's mapped
                let _b = file.as_ptr().read_volatile();
                let _b = file.as_ptr().add(6).read_volatile();
                gem5_writefile(buf.as_ptr(), amount as u64, 0, file.as_ptr() as u64);
            }
        }
    }
    amount
}

/// Flushes the cache
///
/// # Safety
///
/// The caller needs to ensure that cfg::TILE_MEM_BASE is mapped and readable. The area needs to be
/// at least 512 KiB large.
pub unsafe fn flush_cache() {
    // * 2 just to be sure (this code is also touching memory)
    let (cacheline_size, cache_size) = match env!("M3_TARGET") {
        "hw" | "hw23" => (64, 512 * 1024 * 2),
        _ => (64, (32 + 256) * 1024 * 2),
    };

    // ensure that we replace all cachelines in cache
    let mut addr = cfg::TILE_MEM_BASE.as_ptr::<u64>();
    unsafe {
        let end = addr.add(cache_size / 8);
        while addr < end {
            let _val = addr.read_volatile();
            addr = addr.add(cacheline_size / 8);
        }
    }

    #[cfg(all(
        not(target_arch = "riscv32"),
        any(M3_TARGET = "hw", M3_TARGET = "hw23")
    ))]
    unsafe {
        core::arch::asm!("fence.i");
    }
}

pub fn shutdown() -> ! {
    if env::boot().platform == env::Platform::Gem5 {
        #[cfg(not(M3_LX = "1"))]
        unsafe {
            gem5_shutdown(0)
        };
    }
    else {
        // wfi is actually not supported, but it makes the instruction trace stop
        #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
        unsafe {
            core::arch::asm!("1: wfi", "j 1b");
        }
    }
    unreachable!();
}
