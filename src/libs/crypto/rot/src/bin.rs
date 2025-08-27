/*
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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

use crate::MEM_OFFSET;
use base::io::LogFlags;
use base::log;
use base::mem::GlobOff;
use base::tcu::TCU;
use cfg_if::cfg_if;

cfg_if! {
    // note: needs to be in sync with the memory areas in ld.conf
    if #[cfg(any(M3_TARGET = "hw23", M3_TARGET = "hw"))] {
        pub const BROM_NEXT_ADDR: usize = MEM_OFFSET + 0x10000;
        pub const BLAU_NEXT_ADDR: usize = MEM_OFFSET + 0x49000;
        pub const ROSA_NEXT_ADDR: usize = MEM_OFFSET + 0x6000;
    }
    else {
        pub const BROM_NEXT_ADDR: usize = MEM_OFFSET + 0x5000;
        pub const BLAU_NEXT_ADDR: usize = MEM_OFFSET + 0x2A000;
        pub const ROSA_NEXT_ADDR: usize = MEM_OFFSET + 0x69000;
    }
}

/// Load a binary from flash into memory.
///
/// # Safety
/// The caller must ensure that the address is valid and that it comes after
/// any currently used memory location. **Currently no maximum size check
/// exists so the binary will potentially overwrite anything after the load
/// address.**
pub unsafe fn load_bin(addr: usize, bin: &crate::SimpleBinaryCfg) -> &'static [u8] {
    let size = bin.size as usize;
    let ptr = addr as *mut u8;
    log!(
        LogFlags::Info,
        "Loaded binary for next layer to {:#x} .. {:#x}",
        addr,
        addr + size,
    );
    TCU::read(crate::FLASH_EP, ptr, size, bin.flash_offset as GlobOff)
        .expect("Failed to load RoT binary");
    core::slice::from_raw_parts(ptr, size)
}
