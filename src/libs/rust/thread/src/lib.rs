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

#![no_std]
#![allow(internal_features)]
#![feature(core_intrinsics)]
#![feature(hint_assert_unchecked)]
#![feature(new_uninit)]

use base::alloc::alloc;
use base::boxed::Box;
use base::cell::{LazyStaticRefCell, Ref, StaticCell};
use base::col::{ArrayVec, BoxList};
use base::impl_boxitem;
use base::io::LogFlags;
use base::libc;
use base::log;
use base::mem::{self, VirtAddr};
use base::tcu;
use base::{cfg, const_assert};
use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::ptr::{null_mut, slice_from_raw_parts, NonNull};

pub type Event = u64;

const MAX_MSG_SIZE: usize = 1024;

mod refs;

pub use refs::{AsyncLock, Downgradable, NonWeak, StrongRc, TempRc, Upgradable, WeakRc};

#[cfg(target_arch = "x86_64")]
#[derive(Default)]
#[repr(C, align(8))]
pub struct Regs {
    rbx: usize,
    rsp: usize,
    rbp: usize,
    r12: usize,
    r13: usize,
    r14: usize,
    r15: usize,
    rflags: usize,
    rdi: usize,
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
#[derive(Default)]
#[repr(C, align(8))]
pub struct Regs {
    a0: usize,
    ra: usize,
    sp: usize,
    fp: usize,
    s1: usize,
    s2: usize,
    s3: usize,
    s4: usize,
    s5: usize,
    s6: usize,
    s7: usize,
    s8: usize,
    s9: usize,
    s10: usize,
    s11: usize,
}

/// Initialize the thread
///
/// # SAFETY
///
/// The thread stack pointer must be valid and aligned.
#[cfg(target_arch = "x86_64")]
unsafe fn thread_init(thread: &mut Thread, func_addr: VirtAddr, arg: usize) {
    // The x86-64 stack pointer is aligned to 16 byte before the call instruction.
    // Because the call instruction pushes the return address, the stack pointer alignment is
    // deliberately off afterwards.
    // Hence, we need to push the return address to the top of the stack and have the stack pointer
    // point to it.
    // SAFETY: The caller assures that the pointer is valid.
    let stack = &mut *thread.stack;
    // SAFETY: The caller assures that the stack top is naturally aligned.
    let top = stack.as_mut_ptr_range().end.cast::<usize>();
    // put argument in rdi and function to return to on the stack
    thread.regs.rdi = arg;
    let top = top.sub(1);
    top.write(func_addr.as_local());
    thread.regs.rsp = top as usize;
    thread.regs.rbp = thread.regs.rsp;
    thread.regs.rflags = 0x200; // enable interrupts
}

/// Initialize the thread
///
/// # SAFETY
///
/// The thread stack pointer must be valid and aligned.
#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
unsafe fn thread_init(thread: &mut Thread, func_addr: VirtAddr, arg: usize) {
    // The stack pointer is 16-byte aligned on both architectures.
    // SAFETY: The caller assures that the pointer is valid.
    let stack = &mut *thread.stack;
    // SAFETY: The caller assures that the stack top is naturally aligned.
    let top = stack.as_mut_ptr_range().end.cast::<usize>();
    thread.regs.a0 = arg;
    // The stack pointer is allowed to point outside of the stack because RISCV subtracts the stack
    // pointer before writing to it.
    thread.regs.sp = top as usize;
    thread.regs.fp = 0;
    thread.regs.ra = func_addr.as_local();
}

fn alloc_id() -> u32 {
    static NEXT_ID: StaticCell<u32> = StaticCell::new(0);
    NEXT_ID.set(NEXT_ID.get() + 1);
    NEXT_ID.get()
}

const MAX_EVENTS: usize = 5;

type Stack = [MaybeUninit<u8>; cfg::STACK_SIZE];

pub struct Thread {
    prev: Option<NonNull<Thread>>,
    next: Option<NonNull<Thread>>,
    id: u32,
    regs: Regs,
    stack: *mut Stack,
    events: ArrayVec<Event, MAX_EVENTS>,
    has_msg: bool,
    msg: [mem::MaybeUninit<u64>; MAX_MSG_SIZE / 8],
}

impl_boxitem!(Thread);

extern "C" {
    fn thread_switch_async(o: *mut Regs, n: *mut Regs);
}

impl Thread {
    fn new_main() -> Box<Self> {
        Box::new(Thread {
            prev: None,
            next: None,
            id: alloc_id(),
            regs: Regs::default(),
            stack: null_mut(),
            events: Default::default(),
            has_msg: false,
            // safety: will only be safe to access if `has_msg` is true
            msg: unsafe { mem::MaybeUninit::uninit().assume_init() },
        })
    }

    pub fn new(func_addr: VirtAddr, arg: usize) -> Box<Self> {
        let stack_layout = get_stack_layout();
        assert_ne!(stack_layout.size(), 0);
        // Create an uninitialized array on the heap without copying.
        // SAFETY: The layout size is not zero.
        let stack = unsafe { alloc::alloc(stack_layout) };
        if stack.is_null() {
            alloc::handle_alloc_error(stack_layout);
        }
        // SAFETY: We just allocated the stack with the proper size
        let stack = stack.cast();
        let mut thread = Box::new(Thread {
            prev: None,
            next: None,
            id: alloc_id(),
            regs: Regs::default(),
            stack,
            events: Default::default(),
            has_msg: false,
            // safety: will only be safe to access if `has_msg` is true
            msg: unsafe { mem::MaybeUninit::uninit().assume_init() },
        });

        log!(LogFlags::LibThread, "Created thread {}", thread.id);

        // SAFETY: We just sucessfully allocated the stack with proper alignment.
        unsafe {
            thread_init(&mut thread, func_addr, arg);
        }

        thread
    }

    pub fn is_main(&self) -> bool {
        self.stack.is_null()
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn fetch_msg(&mut self) -> Option<&'static tcu::Message> {
        if mem::replace(&mut self.has_msg, false) {
            // safety: has_msg is true and we trust the TCU
            unsafe {
                let header = self.msg.as_ptr().cast::<tcu::Header>();
                let length = (*header).length();
                // Add length information to pointer.
                let msg: *const [()] = slice_from_raw_parts(self.msg.as_ptr().cast::<()>(), length);
                Some(&*(msg as *const tcu::Message))
            }
        }
        else {
            None
        }
    }

    fn subscribe(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Unsubscribe to the latest event
    ///
    /// # Panics
    ///
    /// Panics if the latest event does not match the supplied `event`.
    fn unsubscribe(&mut self, event: Event) {
        assert_eq!(
            self.events.pop(),
            event,
            "unsubscribed to unexpected event (maybe you got the order wrong?)"
        );
    }

    fn trigger_event(&mut self, event: Event) -> bool {
        self.events.iter().any(|&e| e == event)
    }

    fn set_msg(&mut self, msg: &tcu::Message) {
        let size = msg.header.length() + mem::size_of::<tcu::Header>();
        self.has_msg = true;
        // safety: we trust the TCU
        unsafe {
            libc::memcpy(
                self.msg.as_ptr() as *mut libc::c_void,
                msg as *const tcu::Message as *const libc::c_void,
                size,
            );
        }
    }
}

/// Return the allocation layout of the properly-aligned stack
fn get_stack_layout() -> Layout {
    // The top of the stack needs to be properly aligned for the stack pointer if the stack
    // grows downwards.
    // Given that the base is properly aligned, this assert checks that the top is also aligned.
    const_assert!(!libc::GROWS_DOWNWARDS || cfg::STACK_SIZE % libc::STACK_ALIGN == 0);

    Layout::new::<Stack>().align_to(libc::STACK_ALIGN).unwrap()
}

impl Drop for Thread {
    fn drop(&mut self) {
        if !self.stack.is_null() {
            // SAFETY: We created the stack with this layout.
            unsafe {
                alloc::dealloc(self.stack.cast(), get_stack_layout());
            }
        }
        log!(LogFlags::LibThread, "Thread {} destroyed", self.id);
    }
}

struct ThreadManager {
    current: Box<Thread>,
    ready: BoxList<Thread>,
    block: BoxList<Thread>,
    sleep: BoxList<Thread>,
}

static TMNG: LazyStaticRefCell<ThreadManager> = LazyStaticRefCell::default();

pub fn init() {
    TMNG.set(ThreadManager::new());
}

impl ThreadManager {
    fn new() -> Self {
        ThreadManager {
            current: Thread::new_main(),
            ready: BoxList::new(),
            block: BoxList::new(),
            sleep: BoxList::new(),
        }
    }

    fn notify(&mut self, event: Event, msg: Option<&tcu::Message>) {
        let mut it = self.block.iter_mut();
        while let Some(t) = it.next() {
            if t.trigger_event(event) {
                if let Some(m) = msg {
                    t.set_msg(m);
                }
                log!(
                    LogFlags::LibThread,
                    "Waking up thread {} for event {:#x}",
                    t.id,
                    event
                );
                let t = it.remove();
                self.ready.push_back(t.unwrap());
            }
        }
    }

    fn get_next(&mut self) -> Option<Box<Thread>> {
        if !self.ready.is_empty() {
            self.ready.pop_front()
        }
        else {
            self.sleep.pop_front()
        }
    }
}

pub fn cur() -> Ref<'static, Box<Thread>> {
    Ref::map(TMNG.borrow(), |tmng| &tmng.current)
}

pub fn thread_count() -> usize {
    let tmng = TMNG.borrow();
    tmng.ready.len() + tmng.block.len() + tmng.sleep.len()
}

pub fn ready_count() -> usize {
    TMNG.borrow().ready.len()
}

pub fn blocked_count() -> usize {
    TMNG.borrow().block.len()
}

pub fn sleeping_count() -> usize {
    TMNG.borrow().sleep.len()
}

pub fn fetch_msg() -> Option<&'static tcu::Message> {
    TMNG.borrow_mut().current.fetch_msg()
}

pub fn add_thread(func_addr: VirtAddr, arg: usize) {
    TMNG.borrow_mut()
        .sleep
        .push_back(Thread::new(func_addr, arg));
}

pub fn remove_thread() {
    TMNG.borrow_mut().sleep.pop_front().unwrap();
}

/// Use the bits of the address as an event.
#[inline(always)]
pub fn ptr_to_event<T>(ptr: NonNull<T>) -> Event {
    const_assert!(usize::BITS <= Event::BITS);
    ptr.as_ptr() as Event
}

pub fn alloc_event() -> Event {
    static NEXT_EVENT: StaticCell<Event> = StaticCell::new(0);
    // if we have no other threads available, don't use events
    if sleeping_count() == 0 {
        0
    }
    // otherwise, use a unique number
    else {
        NEXT_EVENT.set(NEXT_EVENT.get() + 1);
        NEXT_EVENT.get()
    }
}

pub fn wait_for_async(event: Event) {
    let mut tmng = TMNG.borrow_mut();
    let next = tmng.get_next().unwrap();

    log!(
        LogFlags::LibThread,
        "Thread {} waits for {:#x}, switching to {}",
        tmng.current.id,
        event,
        next.id
    );

    refs::check_async_call();

    let mut cur = mem::replace(&mut tmng.current, next);
    cur.subscribe(event);

    // safety: moving between two lists is fine
    unsafe {
        let old = Box::into_raw(cur);
        tmng.block.push_back(Box::from_raw(old));
        let next_ptr = &mut tmng.current.regs as *mut _;
        drop(tmng);

        thread_switch_async(&mut (*old).regs as *mut _, next_ptr);
    }

    let mut tmng = TMNG.borrow_mut();
    // Pop the event we just pushed in subscribe.
    tmng.current.unsubscribe(event);
}

/// Wait for the event and the `awaitables` too.
pub fn wait_many_async(event: Event, awaitables: &[&dyn Awaitable]) {
    let mut tmng = TMNG.borrow_mut();
    for awaitable in awaitables {
        if awaitable.ready() {
            return;
        }
    }
    for awaitable in awaitables {
        tmng.current.subscribe(awaitable.event());
    }
    drop(tmng);

    wait_for_async(event);

    let mut tmng = TMNG.borrow_mut();
    for awaitable in awaitables.iter().rev() {
        tmng.current.unsubscribe(awaitable.event())
    }
}

pub fn notify(event: Event, msg: Option<&tcu::Message>) {
    TMNG.borrow_mut().notify(event, msg)
}

pub fn try_yield_async() {
    let mut tmng = TMNG.borrow_mut();
    match tmng.ready.pop_front() {
        None => {},
        Some(next) => {
            log!(
                LogFlags::LibThread,
                "Yielding from {} to {}",
                tmng.current.id,
                next.id
            );

            refs::check_async_call();

            let cur = mem::replace(&mut tmng.current, next);

            // safety: moving between two lists is fine
            unsafe {
                let old = Box::into_raw(cur);
                tmng.sleep.push_back(Box::from_raw(old));
                let next_ptr = &mut tmng.current.regs as *mut _;
                drop(tmng);

                thread_switch_async(&mut (*old).regs as *mut _, next_ptr);
            }
        },
    }
}

/// Stops the current thread and switches to a sleeping thread
///
/// Does nothing if no sleeping thread is available.
/// The current thread object is leaked.
pub fn stop_async() {
    let mut tmng = TMNG.borrow_mut();
    if let Some(next) = tmng.get_next() {
        log!(
            LogFlags::LibThread,
            "Stopping thread {}, switching to {}",
            tmng.current.id,
            next.id
        );

        refs::check_async_call();

        let mut cur = mem::replace(&mut tmng.current, next);

        let next_ptr = &mut tmng.current.regs as *mut _;
        drop(tmng);

        unsafe {
            thread_switch_async(&mut cur.regs as *mut _, next_ptr);
        }
    }
}

/// Something that can be awaited for until ready using an event.
pub trait Awaitable {
    fn ready(&self) -> bool;
    fn event(&self) -> Event;
}
