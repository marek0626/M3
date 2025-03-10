/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2020 Nils Asmussen, Barkhausen Institut
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

//! Contains functions and types found in the libc

#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum c_void {
    // Two dummy variants so the #[repr] attribute can be used.
    #[doc(hidden)]
    __variant1,
    #[doc(hidden)]
    __variant2,
}

extern "C" {
    pub fn memcpy(dst: *mut c_void, src: *const c_void, len: usize) -> *mut c_void;
    pub fn memset(dst: *mut c_void, val: i32, len: usize) -> *mut c_void;
    pub fn memzero(dst: *mut c_void, len: usize);
    pub fn strlen(s: *const i8) -> usize;
}

/// Maximum scalar alignment requirement.
///
/// This should be compatible with the `max_align_t` struct of the libC.
/// It is the alignment guaranteed for `malloc` allocations.
///
/// # Sources
///
/// - [_RISC-V_](https://github.com/riscv-non-isa/riscv-elf-psabi-doc/blob/2484f950a551c653f1823f1bd11926bf5a57fae3/riscv-cc.adoc?plain=1#L617)
#[cfg(any(
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "x86_64",
))]
pub const MAX_ALIGN: usize = 16;
