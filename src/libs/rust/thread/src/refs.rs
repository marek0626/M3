/*
 * Copyright (C) 2024 Nils Asmussen, Barkhausen Institut
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

//! This module provides the types `AsyncRc` and `AsyncWeak` to help dealing with destructible
//! objects and async calls via `crate::wait_for`.
//!
//! When performing async calls, we might not want to hold strong references (`Rc`) to objects
//! during such calls to ensure the objects stays valid, because:
//! 1. We lack a generic way to check whether involved objects were destroyed during async calls.
//!    Using other means like calling a method on the object is possible, but easy to forget and
//!    forces objects to be "invalidatible".
//! 2. It leads to unpredictable delays for object destructions. For example, a revoke could leave
//!    many resources around even though it reported completion to userspace.
//!
//! This module provides an alternative via the `AsyncRc` and `AsyncWeak` types: guard access to
//! objects via `AsyncRc` and enforce that we either drop them before an async call or convert them
//! to `AsyncWeak`. Similarly, `AsyncLock` prevents that async calls are done while it is held.

use core::ops::Deref;

use base::backtrace;
use base::cell::{StaticCell, StaticRefCell};
use base::io::LogFlags;
use base::log;
use base::mem::VirtAddr;
use base::rc::{Rc, Weak};

/// Log increments/decrements for `AsyncRc`s
const LOGGING: bool = false;
/// Enable (costly) debug infrastructure for `AsyncRc` to show where they were constructed
const DEBUG: bool = false;
/// For DEBUG: the maximum length of each backtrace
const MAX_TRACE_LEN: usize = 8;
/// For DEBUG: the maximum number of backtraces to store
const MAX_TRACES: usize = 8;

static OWNED_REFS: StaticCell<u64> = StaticCell::new(0);
static REF_TRACES: StaticRefCell<Traces> = StaticRefCell::new(Traces::new());

#[derive(Copy, Clone)]
struct Trace {
    addrs: [VirtAddr; MAX_TRACE_LEN],
}

struct Traces {
    traces: [Trace; MAX_TRACES],
    pos: usize,
}

impl Traces {
    const fn new() -> Self {
        Self {
            traces: [Trace {
                addrs: [VirtAddr::null(); MAX_TRACE_LEN],
            }; MAX_TRACES],
            pos: 0,
        }
    }

    fn push(&mut self) {
        if self.pos < self.traces.len() {
            let n = backtrace::collect(&mut self.traces[self.pos].addrs);
            for i in n..MAX_TRACE_LEN {
                self.traces[self.pos].addrs[i] = VirtAddr::null();
            }
        }
        self.pos += 1;
    }

    fn pop(&mut self) {
        self.pos -= 1;
    }
}

fn inc_owned_refs() {
    OWNED_REFS.set(OWNED_REFS.get() + 1);
    if LOGGING {
        log!(LogFlags::Info, "owned-refs ++ -> {}", OWNED_REFS.get());
    }
    if DEBUG {
        REF_TRACES.borrow_mut().push();
    }
}

fn dec_owned_refs() {
    assert!(OWNED_REFS.get() > 0);
    OWNED_REFS.set(OWNED_REFS.get() - 1);
    if LOGGING {
        log!(LogFlags::Info, "owned-refs -- -> {}", OWNED_REFS.get());
    }
    if DEBUG {
        REF_TRACES.borrow_mut().pop();
    }
}

pub(crate) fn check_async_call() {
    if OWNED_REFS.get() != 0 {
        log!(
            LogFlags::Error,
            "Async call with {} owned reference(s)",
            OWNED_REFS.get()
        );
        if DEBUG {
            let traces = REF_TRACES.borrow();
            log!(LogFlags::Error, "  acquired at these points:");
            for i in 0..traces.pos.min(MAX_TRACES) {
                for j in 0..MAX_TRACE_LEN {
                    if traces.traces[i].addrs[j] == VirtAddr::null() {
                        break;
                    }
                    log!(
                        LogFlags::Error,
                        "    {:#x}",
                        traces.traces[i].addrs[j].as_local()
                    );
                }
                log!(LogFlags::Error, "");
            }
        }
        panic!("Stopping here");
    }
}

/// A weak reference that can be held across async calls
///
/// The `AsyncWeak` type can only be constructed from an `AsyncRc` and can, in contrast to
/// `AsyncRc` be hold across async calls. Afterwards, it can be upgraded back into an `AsyncRc`, if
/// the object has not been destroyed in the meantime.
pub struct AsyncWeak<T> {
    obj: Weak<T>,
}

impl<T> Default for AsyncWeak<T> {
    fn default() -> Self {
        Self {
            obj: Default::default(),
        }
    }
}

impl<T> Clone for AsyncWeak<T> {
    fn clone(&self) -> Self {
        Self {
            obj: self.obj.clone(),
        }
    }
}

impl<T> AsyncWeak<T> {
    /// Returns true if the reference is valid so that `upgrade` will succeed
    pub fn can_upgrade(&self) -> bool {
        self.obj.strong_count() > 0
    }

    /// Tries to upgrade the reference into a `AsyncRc`
    ///
    /// This can fail if the underlying object was destroyed, in which case `None` is returned.
    pub fn upgrade(&self) -> Option<AsyncRc<T>> {
        self.obj.upgrade().map(|o| AsyncRc::new(o))
    }
}

/// A strong reference for in-between async calls
///
/// An `AsyncRc` provides access to an underlying object of type `T`, but holding an `AsyncRc` does
/// not allow async calls (that is, `crate::wait_for` will panic). Performing an async call
/// requires to either drop all `AsyncRc`s or convert them into `AsyncWeak`s. The latter allows to
/// upgrade them back into `AsyncRc`s after the async call, but in a checked way. That is, if the
/// object was destroyed in the meantime, the upgrade will fail.
pub struct AsyncRc<T> {
    obj: Rc<T>,
}

impl<T> AsyncRc<T> {
    // Creates a new `AsyncRc` from given reference-counted object
    pub fn new(obj: Rc<T>) -> Self {
        inc_owned_refs();
        Self { obj }
    }

    /// Returns a reference to the inner `Rc`
    ///
    /// # Safety
    ///
    /// If the inner T can be destroyed, the caller cannot keep the Rc across async calls.
    pub unsafe fn inner(&self) -> &Rc<T> {
        &self.obj
    }

    /// Returns true if the underlying pointers of `self` and `other` are equal
    pub fn ptr_eq(&self, other: &AsyncRc<T>) -> bool {
        Rc::ptr_eq(&self.obj, &other.obj)
    }

    /// Downgrades this `AsyncRc` into a `AsyncWeak`
    pub fn downgrade(self) -> AsyncWeak<T> {
        // count will be decreased in drop of self
        AsyncWeak {
            obj: Rc::downgrade(&self.obj),
        }
    }
}

impl<T> Clone for AsyncRc<T> {
    fn clone(&self) -> Self {
        Self::new(self.obj.clone())
    }
}

impl<T> Drop for AsyncRc<T> {
    fn drop(&mut self) {
        dec_owned_refs();
    }
}

impl<T> Deref for AsyncRc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.obj.deref()
    }
}

/// A lock for async calls
///
/// Holding an instance of `AsyncLock` prevents that async calls are done.
pub struct AsyncLock;

impl AsyncLock {
    /// Creates a new `AsyncLock`
    pub fn new() -> Self {
        inc_owned_refs();
        Self
    }
}

impl Default for AsyncLock {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AsyncLock {
    fn drop(&mut self) {
        dec_owned_refs();
    }
}
