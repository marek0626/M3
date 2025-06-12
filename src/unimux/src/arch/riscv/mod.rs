/*
 * Copyright (C) 2020-2022 Nils Asmussen, Barkhausen Institut
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

use base::cell::StaticCell;
use base::errors::Code;
use base::io::LogFlags;
use base::kif::tilemux;
use base::libc;
use base::mem::{size_of, MaybeUninit};
use base::{log, read_csr, write_csr};

use num_enum::{FromPrimitive, IntoPrimitive};

use crate::activities;

extern "C" {
    fn sleep_once();
}

pub type State = isr::State;

#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive, FromPrimitive)]
#[repr(usize)]
enum FSMode {
    #[default]
    OFF     = 0,
    INITIAL = 1,
    CLEAN   = 2,
    DIRTY   = 3,
}

fn get_fpu_mode(status: usize) -> FSMode {
    FSMode::from((status >> 13) & 0x3)
}

fn set_fpu_mode(mut status: usize, mode: FSMode) -> usize {
    status &= !(0x3 << 13);
    status | (mode as usize) << 13
}

pub fn init_state(state: &mut State, entry: usize, sp: usize) {
    state.epc = entry;
    state.r[1] = sp;
    state.status = read_csr!("sstatus");
    state.status &= !(1 << 8); // user mode
    state.status |= 1 << 5; // interrupts enabled
    state.status = set_fpu_mode(state.status, FSMode::CLEAN);
}

pub fn init_fpu() {
    // enable FPU so that we can save/restore the FPU registers
    write_csr!("sstatus", set_fpu_mode(read_csr!("sstatus"), FSMode::CLEAN));
}
