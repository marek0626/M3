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

pub const BROM_NEXT_ADDR: usize = MEM_OFFSET + 0x3000;
#[cfg(target_arch = "riscv32")]
pub const BLAU_NEXT_ADDR: usize = MEM_OFFSET + 0x20000;
#[cfg(target_arch = "riscv64")]
pub const BLAU_NEXT_ADDR: usize = MEM_OFFSET + 0x18000;
#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
pub const ROSA_ADDR: usize = BLAU_NEXT_ADDR;
#[cfg(target_arch = "riscv32")]
pub const ROSA_NEXT_ADDR: usize = MEM_OFFSET + 0x12000; // TileMux
#[cfg(target_arch = "riscv64")]
pub const ROSA_NEXT_ADDR: usize = MEM_OFFSET + 0x3000; // TileMux

pub const ROSA_ROTS_NEXT_ADDR: usize = ROSA_ADDR + 0x30000; // 0x3_0000 is approx rosa text+bss

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
    TCU::read(crate::FLASH_EP, ptr, size, bin.flash_offset as GlobOff)
        .expect("Failed to load RoT binary");
    log!(
        LogFlags::RoTBoot,
        "Loaded binary for next layer: {} bytes",
        size
    );
    core::slice::from_raw_parts(ptr, size)
}
