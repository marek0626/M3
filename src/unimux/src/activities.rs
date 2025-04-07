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

use base::boxed::Box;
use base::cell::{LazyStaticUnsafeCell, StaticCell, StaticRefCell, StaticUnsafeCell};
use base::cfg;
use base::col::{BoxList, Vec};
use base::errors::Error;
use base::impl_boxitem;
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{GlobAddr, GlobOff, MsgBuf, PhysAddr, PhysAddrRaw, VirtAddr};
use base::tcu;
use base::time::TimeInstant;
use base::tmif;
use base::util::math;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use paging::{ArchPaging, Paging};
pub type Id = paging::ActId;

use crate::arch;
use crate::pex_env;
use crate::Code;
use mux::{helper, sendqueue};

use isr::{ISRArch, ISR};

#[derive(PartialEq)]
enum ActivityState {
    UNREADY,
    READY,
    STARTED,
    BLOCKED,
}

pub struct Activity {
    state: ActivityState,
    prev: Option<NonNull<Activity>>,
    next: Option<NonNull<Activity>>,
    #[cfg(any(
        target_arch = "riscv64",
        target_arch = "riscv32",
        target_arch = "x86_64"
    ))]
    user_state: arch::State,
    user_state_addr: VirtAddr,
    act_reg: tcu::Reg,
    eps_start: tcu::EpId,
    cmd: helper::TCUCmdState,
    has_refs: bool,
}

/// A reference to an activity that ensures at runtime that there is always just one reference to
/// each activity at a time.
pub struct ActivityRef<'a> {
    act: &'a mut Activity,
}

impl<'m> ActivityRef<'m> {
    fn new(act: &'m mut Activity) -> Self {
        assert!(!act.has_refs);
        act.has_refs = true;
        Self { act }
    }
}

impl Drop for ActivityRef<'_> {
    //NMG Make the compiler cooperate with UB
    #[inline(never)]
    fn drop(&mut self) {
        self.act.has_refs = false;
    }
}

impl Deref for ActivityRef<'_> {
    type Target = Activity;

    fn deref(&self) -> &Self::Target {
        self.act
    }
}

impl DerefMut for ActivityRef<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.act
    }
}

impl_boxitem!(Activity);

static OUR: LazyStaticUnsafeCell<Box<Activity>> = LazyStaticUnsafeCell::default();
static IDLE: LazyStaticUnsafeCell<Box<Activity>> = LazyStaticUnsafeCell::default();
static USER: LazyStaticUnsafeCell<Box<Activity>> = LazyStaticUnsafeCell::default();
static CUR: StaticCell<Option<Id>> = StaticCell::new(None);

pub fn try_cur() -> Option<Id> {
    // safety: we check at runtime whether a reference to this activity already exists
    CUR.get()
}

pub fn cur() -> Id {
    try_cur().unwrap()
}

pub fn init() {
    extern "C" {
        static _bss_end: usize;
    }

    // safety: there are no other references to IDLE or OUR yet
    unsafe {
        OUR.set(Box::new(Activity::new(kif::tilemux::ACT_ID, 0)));
        IDLE.set(Box::new(Activity::new(kif::tilemux::IDLE_ID, 0)));
    }

    Paging::disable();
}

pub fn set_user(id: u64, eps_start: tcu::EpId) {
    // As a 'unimux' we shouldn't be receiving multiple activity starts.
    assert!(!USER.is_some());
    unsafe {
        USER.set(Box::new(Activity::new(id, eps_start)));
    }
}

pub fn user_is_some() -> bool {
    USER.is_some()
}

pub fn user() -> ActivityRef<'static> {
    ActivityRef::new(unsafe { USER.get_mut() })
}

pub fn idle() -> ActivityRef<'static> {
    // safety: we check at runtime whether a reference to this activity already exists
    ActivityRef::new(unsafe { IDLE.get_mut() })
}

pub fn get_mut(id: Id) -> Option<ActivityRef<'static>> {
    let act_id = id as tcu::ActId;
    if act_id == kif::tilemux::ACT_ID as tcu::ActId {
        Some(our())
    }
    else if act_id == kif::tilemux::IDLE_ID as tcu::ActId {
        Some(idle())
    }
    else {
        if USER.is_some() {
            Some(user())
        }
        else {
            None
        }
    }
}

pub fn our() -> ActivityRef<'static> {
    // safety: we check at runtime whether a reference to this activity already exists
    ActivityRef::new(unsafe { OUR.get_mut() })
}

fn block(mut act: Box<Activity>) {
    act.state = ActivityState::BLOCKED;
    //BLK.borrow_mut().push_back(act);
}

pub fn set_cur(next: Id) {
    CUR.set(Some(next));
}

pub fn update_our_activity() {
    let act = tcu::TCU::get_cur_activity();
    our().set_activity_reg(act);
}

pub fn stop_activity(status: Code) {
    user().block(true);

    let act = tcu::TCU::get_cur_activity();
    user().set_activity_reg(act);

    let old = tcu::TCU::xchg_activity(our().activity_reg()).unwrap();

    let mut msg_buf = MsgBuf::borrow_def();
    base::build_vmsg!(msg_buf, kif::tilemux::Calls::Exit, kif::tilemux::Exit {
        act_id: user().id() as tcu::ActId,
        status,
    });
    sendqueue::send(&msg_buf).unwrap();
}

impl Activity {
    pub fn new(id: Id, eps_start: tcu::EpId) -> Self {
        Activity {
            prev: None,
            next: None,
            act_reg: id,
            state: ActivityState::UNREADY,
            user_state: arch::State::default(),
            user_state_addr: VirtAddr::null(),
            eps_start,
            cmd: helper::TCUCmdState::new(),
            has_refs: false,
        }
    }

    pub fn id(&self) -> Id {
        self.act_reg & 0xFFFF
    }

    pub fn activity_reg(&self) -> tcu::Reg {
        self.act_reg
    }

    pub fn set_activity_reg(&mut self, val: tcu::Reg) {
        self.act_reg = val;
    }

    pub fn msgs(&self) -> u16 {
        (self.act_reg >> 16) as u16
    }

    pub fn has_msgs(&self) -> bool {
        self.msgs() != 0
    }

    pub fn add_msg(&mut self) {
        self.act_reg += 1 << 16;
    }

    pub fn rem_msgs(&mut self, count: u16) {
        assert!(self.msgs() >= count);
        self.act_reg -= (count as u64) << 16;
    }

    pub fn user_state(&mut self) -> &mut arch::State {
        &mut self.user_state
    }

    pub fn user_state_addr(&mut self) -> VirtAddr {
        self.user_state_addr
    }

    pub fn block(&mut self, state: bool) {
        self.state = match state {
            true => ActivityState::BLOCKED,
            false => ActivityState::STARTED,
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.state == ActivityState::BLOCKED
    }

    pub fn is_ready(&self) -> bool {
        self.state == ActivityState::READY
    }

    pub fn started(&mut self) {
        self.state = ActivityState::STARTED
    }

    pub fn start(&mut self) {
        assert!(self.user_state_addr.is_null());
        let entry = crate::env_run;
        if self.id() != kif::tilemux::IDLE_ID {
            extern "C" {
                static baremetal_stack: u8;
            }

            log!(
                LogFlags::MuxActs,
                "Starting Activity {} with entry={:#x}, sp={:#x}",
                self.id(),
                entry as usize,
                unsafe { core::ptr::addr_of!(baremetal_stack) as usize },
            );
            arch::init_state(&mut self.user_state, entry as usize, unsafe {
                core::ptr::addr_of!(baremetal_stack) as usize
            });
        }
        self.user_state_addr = VirtAddr::from(&self.user_state as *const _);
        self.state = ActivityState::READY;
    }

    pub fn switch_to(&self) {
    }
}

fn halt() {
    loop {}
}

impl Drop for Activity {
    fn drop(&mut self) {
        log!(LogFlags::MuxActs, "Destroyed Activity {}", self.id());

        halt();
    }
}
