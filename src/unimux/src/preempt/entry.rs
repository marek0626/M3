/*
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

use base::cell::StaticCell;
use base::io::LogFlags;
use base::libc;
use base::log;
use base::mem;
use base::tcu;

use isr::{ISRArch, ISR};

use crate::exit;
use crate::hdl::activities;
use crate::hdl::cureq;
use crate::hdl::sleep_once;
use crate::hdl::state;
use crate::hdl::timer;
use crate::hdl::tmcalls;
use crate::sidecalls;

static NEED_TIMER: StaticCell<bool> = StaticCell::new(true);
static NEED_SWITCH: StaticCell<bool> = StaticCell::new(false);

pub fn reg_timer_reprogram() {
    NEED_TIMER.set(true);
}

fn idle() {
    activities::set_cur(activities::idle().activity_reg());
    let old = tcu::TCU::xchg_activity(activities::idle().activity_reg()).unwrap();
    // NMG Have to switch the stack so we don't clobber stuff on nested IRQs (?)
    ISR::set_entry_sp(activities::idle().user_state_addr() + mem::size_of::<state::State>());
    activities::get_mut(old).unwrap().set_activity_reg(old);
    sidecalls::check();
    ISR::enable_irqs();
    loop {
        unsafe {
            sleep_once();
        }
    }
}

fn start() {
    let mut user = activities::user();
    activities::set_cur(user.activity_reg());
    tcu::TCU::xchg_activity(user.activity_reg()).unwrap();
    ISR::set_entry_sp(user.user_state_addr() + mem::size_of::<state::State>());

    state::init_fpu();

    log!(
        LogFlags::Debug,
        "switching to user and starting up: reg {} user_state_addr {}",
        user.activity_reg(),
        user.user_state_addr()
    );
    user.started();
}

#[inline]
fn leave(state: &mut state::State) -> *mut libc::c_void {
    let ready = activities::user_is_some() && activities::user().is_ready();

    sidecalls::check();

    if NEED_TIMER.replace(false) {
        timer::reprogram();
    }

    if activities::user_is_some() && activities::user().is_blocked() {
        idle();
    }

    if !ready && activities::user_is_some() && activities::user().is_ready() {
        start();
        activities::user().user_state_addr().as_mut_ptr()
    }
    else {
        state as *mut _ as *mut libc::c_void
    }
}

extern "C" fn unexpected_irq(state: &mut state::State) -> *mut libc::c_void {
    log!(
        LogFlags::Error,
        "Unexpected IRQ with user state:\n{:?}",
        state
    );
    exit(1);
}

extern "C" fn ext_irq(state: &mut state::State) -> *mut libc::c_void {
    match ISR::fetch_irq() {
        isr::IRQSource::TCU(tcu::IRQ::Timer) => {
            let mut user = activities::user();
            ISR::set_entry_sp(user.user_state_addr() + mem::size_of::<state::State>());
            user.set_blocked(false);
            timer::trigger();
            NEED_SWITCH.set(true);
        },

        isr::IRQSource::TCU(tcu::IRQ::CUReq) => {
            if let Some(r) = tcu::TCU::get_cu_req() {
                log!(LogFlags::MuxCUReqs, "Got {:x?}", r);
                cureq::handle(r);
            }
        },

        isr::IRQSource::Ext(id) => {
            panic!("Unexpected external IRQ: {}!", id);
        },
    };

    leave(state)
}

extern "C" fn tmcall(state: &mut state::State) -> *mut libc::c_void {
    log!(LogFlags::MuxCalls, "received irq for tmcall");
    tmcalls::handle_call(state);

    leave(state)
}

pub fn init() {
    let mut idle = activities::idle();
    let state = idle.user_state();
    ISR::init(state);
    idle.start();

    let old = tcu::TCU::xchg_activity(idle.activity_reg()).unwrap();
    activities::our().set_activity_reg(old);
    activities::set_cur(idle.activity_reg());

    // All IRQs start unexpected
    isr::reg_all(unexpected_irq);
    // Handle the mux calls, e.g. wait, exit, yield
    ISR::reg_tm_calls(tmcall);
    // Handle the (very important) message receive as well as PMP failures
    ISR::reg_cu_reqs(ext_irq);
    // Handle timer interrupts, simple
    ISR::reg_timer(ext_irq);
    // Handle other external IRQs by number, particularly things in the Event space.
    ISR::reg_external(ext_irq);
}
