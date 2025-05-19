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

#![no_std]
#![allow(warnings)]

#[allow(unused_extern_crates)]
extern crate heap;

mod activities;
mod arch;
mod cureq;
mod sidecalls;
mod timer;
mod tmcalls;

use base::cell::{Ref, StaticCell, StaticRefCell};
use base::cfg;
use base::env::{self, BootEnv};
use base::errors::{Code, Error};
use base::io::{self, LogFlags};
use base::kif;
use base::libc;
use base::log;
use base::machine;
use base::mem;
use base::serialize::{Deserialize, Serialize};
use base::tcu::{self, TCU};
use mux::{helper, sendqueue};

use core::ptr;

use isr::{ISRArch, ISR};

extern "C" {
    fn __m3_init_libc(argc: i32, argv: *const *const u8, envp: *const *const u8, tls: bool);
    fn __m3_heap_set_area(begin: usize, end: usize);
    fn sleep();
    fn sleep_once();
}

const HEAP_SIZE: usize = 128 * 1024;

// the heap area needs to be page-byte aligned
#[repr(align(4096))]
struct Heap([u64; HEAP_SIZE / mem::size_of::<u64>()]);
#[used]
static mut HEAP: Heap = Heap([0; HEAP_SIZE / mem::size_of::<u64>()]);

static TM_ENV: StaticRefCell<mux::TMEnv> = StaticRefCell::new(mux::TMEnv {
    tile_id: 0,
    tile_desc: kif::TileDesc::new_from(0),
    platform: env::Platform::Gem5,
});

pub fn pex_env() -> Ref<'static, mux::TMEnv> {
    TM_ENV.borrow()
}

pub fn app_env() -> &'static mut env::BaseEnv {
    unsafe { &mut *(cfg::ENV_START.as_mut_ptr()) }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "base::serde")]
pub struct PagefaultMessage {
    pub op: u64,
    pub virt: mem::VirtAddr,
    pub access: u64,
}

#[no_mangle]
pub extern "C" fn abort() {
    exit(1);
}

#[no_mangle]
pub extern "C" fn exit(_code: i32) {
    machine::shutdown();
}

static NEED_TIMER: StaticCell<bool> = StaticCell::new(true);
static NEED_SWITCH: StaticCell<bool> = StaticCell::new(false);

#[inline]
fn leave(state: &mut arch::State) -> *mut libc::c_void {
    sidecalls::check();

    if NEED_TIMER.replace(false) {
        timer::reprogram();
    }

    if (activities::user_is_some() && activities::user().is_blocked()) {
        idle();
    }

    // NMG After this point, using the logger is dangerous because user code
    // could get pre-empted in the middle and result in a double-borrow
    // situation.
    let state = if NEED_SWITCH.replace(false) {
        let mut user = activities::user();
        let old = tcu::TCU::xchg_activity(user.activity_reg()).unwrap();
        user.user_state_addr().as_mut_ptr()
    }
    else if activities::user_is_some() && activities::user().is_ready() {
        crate::switch_user().as_mut_ptr()
    }
    else {
        state as *mut _ as *mut libc::c_void
    };

    state
}

pub fn reg_timer_reprogram() {
    NEED_TIMER.set(true);
}

fn halt() {
    loop {}
}

pub extern "C" fn unexpected_irq(state: &mut arch::State) -> *mut libc::c_void {
    log!(
        LogFlags::Error,
        "Unexpected IRQ with user state:\n{:?}",
        state
    );
    halt();

    leave(state)
}

#[cfg(any(
    target_arch = "riscv64",
    target_arch = "riscv32",
    target_arch = "x86_64"
))]

pub extern "C" fn fpu_ex(state: &mut arch::State) -> *mut libc::c_void {
    panic!("Unexpected FPU exception!");
}

pub extern "C" fn ext_irq(state: &mut arch::State) -> *mut libc::c_void {
    match ISR::fetch_irq() {
        isr::IRQSource::TCU(tcu::IRQ::Timer) => {
            let mut user = activities::user();
            ISR::set_entry_sp(user.user_state_addr() + mem::size_of::<arch::State>());
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

extern "Rust" {
    fn env_run() -> !;
}

pub fn switch_user() -> mem::VirtAddr {
    // NMG This may be a double-init of libc. We'll just have to see what happens here.
    // When we switch into the target we need to switch our activity ID, too, so that our EPs work as expected.
    let old = tcu::TCU::xchg_activity(activities::user().activity_reg()).unwrap();
    let mut user = activities::user();
    crate::arch::init_fpu();
    activities::set_cur(user.activity_reg());
    ISR::set_entry_sp(user.user_state_addr() + mem::size_of::<arch::State>());
    log!(
        LogFlags::Debug,
        "switching to user and starting up: reg {} user_state_addr {}",
        user.activity_reg(),
        user.user_state_addr()
    );
    user.started();
    user.user_state_addr()
}

fn wait_for_init() -> Result<(), Error> {
    // This first time we know the activity register always contanins 'our'
    let old = tcu::TCU::xchg_activity(activities::idle().activity_reg()).unwrap();
    activities::our().set_activity_reg(old);
    activities::set_cur(activities::idle().activity_reg());
    loop {
        sidecalls::check();
        if activities::user_is_some() && activities::user().is_ready() {
            break;
        }
        unsafe {
            sleep_once();
        }
    }
    Ok(())
}

fn idle() {
    activities::set_cur(activities::idle().activity_reg());
    let old = tcu::TCU::xchg_activity(activities::idle().activity_reg()).unwrap();
    // NMG Have to switch the stack so we don't clobber stuff on nested IRQs (?)
    ISR::set_entry_sp(activities::idle().user_state_addr() + mem::size_of::<arch::State>());
    activities::get_mut(old).unwrap().set_activity_reg(old);
    sidecalls::check();
    ISR::enable_irqs();
    loop {
        unsafe {
            // NMG sleep_once knows whether we are on hardware (thus capable of wfi)
            sleep_once();
        }
    }
}

pub extern "C" fn tmcall(state: &mut arch::State) -> *mut libc::c_void {
    log!(LogFlags::MuxCalls, "received irq for tmcall");
    tmcalls::handle_call(state);

    leave(state)
}

#[no_mangle]
pub extern "C" fn init() -> usize {
    // copy the environment from earlier stages
    let rot_env: &BootEnv = unsafe { &*(cfg::ENV_START_ROT.as_ptr()) };
    let rots_env: &mut BootEnv = unsafe { &mut *(cfg::ENV_START.as_mut_ptr()) };
    *rots_env = *rot_env;

    // init our own environment; at this point we can still access app_env, because it is mapped by
    // the gem5 loader for us. afterwards, our address space does not contain that anymore.

    {
        let mut env = TM_ENV.borrow_mut();
        env.tile_id = app_env().boot.tile_id;
        env.tile_desc = kif::TileDesc::new_from(app_env().boot.tile_desc);
        env.platform = app_env().boot.platform;
    }

    unsafe {
        __m3_init_libc(0, ptr::null(), ptr::null(), false);
        __m3_heap_set_area(
            &HEAP.0 as *const u64 as usize,
            &HEAP.0 as *const u64 as usize + mem::size_of_val(&HEAP.0),
        );
    }

    io::init(
        tcu::TileId::new_from_raw(pex_env().tile_id as u16),
        "unimux",
    );

    mux::init(crate::pex_env());
    activities::init();

    let state_top = {
        let mut idle = activities::idle();
        idle.start();
        let state = idle.user_state();
        ISR::init(state);
        state as *const _ as usize + mem::size_of::<arch::State>()
    };

    // All IRQs start unexpected
    isr::reg_all(unexpected_irq);
    // Handle the mux calls, e.g. wait, exit, yield
    ISR::reg_tm_calls(tmcall);
    // Handle illegal intructions
    #[cfg(any(
        target_arch = "riscv64",
        target_arch = "riscv32",
        target_arch = "x86_64"
    ))]
    ISR::reg_illegal_instr(fpu_ex);
    // Handle the (very important) message receive as well as PMP failures
    ISR::reg_cu_reqs(ext_irq);
    // Handle timer interrupts, simple
    ISR::reg_timer(ext_irq);
    // Handle other external IRQs by number, particularly things in the Event space.
    ISR::reg_external(ext_irq);

    sidecalls::basic_handlers_init();

    // NMG After this returns we have switched to the user task mid-interrupt,
    // and when we return we should be working on the new user stack.
    wait_for_init();

    ISR::enable_irqs();
    unsafe {
        env_run();
    }

    state_top
}
