/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
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

pub type State = isr::State;

pub fn init_state(state: &mut State, entry: usize, sp: usize) {
    state.rip = entry;
    state.rsp = sp;
    state.r[8] = 0; // rbp
    state.r[14] = 0xDEAD_BEEF; // set rax to tell crt0 that we've set the SP

    state.rflags = 0x200; // enable interrupts

    // run in user mode
    state.cs = ((isr::Segment::UCode as usize) << 3) | isr::DPL::User as usize;
    state.ss = ((isr::Segment::UData as usize) << 3) | isr::DPL::User as usize;
}
