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

//! This module provides the types `StrongRc`, `TempRc`, and `WeakRc` to help dealing with
//! destructible objects and async calls via `crate::wait_for`.
//!
//! When accessing objects and performing async calls, we might not want to keep these objects
//! alive via strong references (`Rc` or `StrongRc`) across the async call, because:
//! 1. We lack a generic way to check whether involved objects were destroyed during async calls.
//!    Using other means like calling a method on the object is possible, but easy to forget and
//!    forces objects to be "invalidatible".
//! 2. It leads to unpredictable delays for object destructions. For example, a revoke could leave
//!    many resources around even though it reported completion to userspace.
//!
//! This module provides an alternative via the `StrongRc`, `TempRc`, and `WeakRc` types: objects
//! are stored as `StrongRc`, but temporary access to them is only provided via `TempRc`. In
//! contrast to `StrongRc`, `TempRc` cannot be hold across async calls, but needs to be downgraded
//! to `WeakRc` before the call. This forces us to check its validity after the call by needing to
//! upgrading it back to an `TempRc`, which fails if the object was destroyed in the meantime.
//! Similarly, `AsyncLock` prevents that async calls are done while it is held.

use core::cell::Cell;
use core::fmt;
use core::intrinsics;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr::{self, NonNull};
use core::{hint, mem};

use base::backtrace;
use base::boxed::Box;
use base::cell::{StaticCell, StaticRefCell};
use base::io::LogFlags;
use base::log;
use base::mem::VirtAddr;

/// Log increments/decrements for `TempRc`s
const LOGGING: bool = false;
/// Enable (costly) debug infrastructure for `TempRc` to show where they were constructed
const DEBUG: bool = false;
/// For DEBUG: the maximum length of each backtrace
const MAX_TRACE_LEN: usize = 8;
/// For DEBUG: the maximum number of backtraces to store
const MAX_TRACES: usize = 8;

static TEMP_REFS: StaticCell<u64> = StaticCell::new(0);
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

fn inc_temp_refs() {
    TEMP_REFS.set(TEMP_REFS.get() + 1);
    if LOGGING {
        log!(LogFlags::Info, "temp-refs ++ -> {}", TEMP_REFS.get());
    }
    if DEBUG {
        REF_TRACES.borrow_mut().push();
    }
}

fn dec_temp_refs() {
    assert!(TEMP_REFS.get() > 0);
    TEMP_REFS.set(TEMP_REFS.get() - 1);
    if LOGGING {
        log!(LogFlags::Info, "temp-refs -- -> {}", TEMP_REFS.get());
    }
    if DEBUG {
        REF_TRACES.borrow_mut().pop();
    }
}

pub(crate) fn check_async_call() {
    if TEMP_REFS.get() != 0 {
        log!(
            LogFlags::Error,
            "Async call with {} temporary reference(s)",
            TEMP_REFS.get()
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

/// Holds the value
///
/// `StrongRc` directly links to this struct, whereas `WeakRc` links to this struct indirectly
/// via `WeakRcLink`.
struct RcBox<T> {
    strong: Cell<usize>,
    // links back to us; will be invalidated if `Self` is dropped
    weak_link: NonNull<WeakRcLink<T>>,
    value: T,
}

/// Helper to allow accessing the weak count without making any assertions about the data field.
struct WeakRcInner<'a> {
    strong: &'a Cell<usize>,
    weak: &'a Cell<usize>,
}

#[doc(hidden)]
trait StrongInnerPtr {
    fn strong_ref(&self) -> &Cell<usize>;

    #[inline]
    fn strong(&self) -> usize {
        self.strong_ref().get()
    }

    #[inline]
    fn inc_strong(&self) {
        let strong = self.strong();

        // We insert an `assume` here to hint LLVM at an otherwise
        // missed optimization.
        // SAFETY: The reference count will never be zero when this is
        // called.
        unsafe {
            hint::assert_unchecked(strong != 0);
        }

        let strong = strong.wrapping_add(1);
        self.strong_ref().set(strong);

        // We want to abort on overflow instead of dropping the value.
        // Checking for overflow after the store instead of before
        // allows for slightly better code generation.
        if intrinsics::unlikely(strong == 0) {
            intrinsics::abort();
        }
    }

    #[inline]
    fn dec_strong(&self) {
        self.strong_ref().set(self.strong() - 1);
    }
}

#[doc(hidden)]
trait WeakRcInnerPtr {
    fn weak_ref(&self) -> &Cell<usize>;

    #[inline]
    fn weak(&self) -> usize {
        self.weak_ref().get()
    }

    #[inline]
    fn inc_weak(&self) {
        let weak = self.weak();

        // We insert an `assume` here to hint LLVM at an otherwise
        // missed optimization.
        // SAFETY: The reference count will never be zero when this is
        // called.
        unsafe {
            hint::assert_unchecked(weak != 0);
        }

        let weak = weak.wrapping_add(1);
        self.weak_ref().set(weak);

        // We want to abort on overflow instead of dropping the value.
        // Checking for overflow after the store instead of before
        // allows for slightly better code generation.
        if intrinsics::unlikely(weak == 0) {
            intrinsics::abort();
        }
    }

    #[inline]
    fn dec_weak(&self) {
        self.weak_ref().set(self.weak() - 1);
    }
}

impl<T> StrongInnerPtr for RcBox<T> {
    #[inline(always)]
    fn strong_ref(&self) -> &Cell<usize> {
        &self.strong
    }
}

impl<T> WeakRcInnerPtr for RcBox<T> {
    #[inline(always)]
    fn weak_ref(&self) -> &Cell<usize> {
        // safety: the weak_link is always valid while the StrongRc exists
        unsafe { &(*self.weak_link.as_ptr()).weak }
    }
}

impl<'a> StrongInnerPtr for WeakRcInner<'a> {
    #[inline(always)]
    fn strong_ref(&self) -> &Cell<usize> {
        self.strong
    }
}

impl<'a> WeakRcInnerPtr for WeakRcInner<'a> {
    #[inline(always)]
    fn weak_ref(&self) -> &Cell<usize> {
        self.weak
    }
}

impl<T> WeakRcInnerPtr for WeakRcLink<T> {
    #[inline(always)]
    fn weak_ref(&self) -> &Cell<usize> {
        &self.weak
    }
}

#[inline(always)]
#[allow(clippy::useless_transmute)]
fn dangling<T>() -> NonNull<T> {
    // safety: same as in core::rc
    unsafe { NonNull::new_unchecked(mem::transmute(usize::MAX)) }
}

#[inline(always)]
fn is_dangling<T>(ptr: *const T) -> bool {
    ptr.cast::<()>() as usize == usize::MAX
}

/// A link to `RcBox` used for `WeakRc`
///
/// The link is valid if the `RcBox` exists
struct WeakRcLink<T> {
    ptr: NonNull<RcBox<T>>,
    weak: Cell<usize>,
}

impl<T> WeakRcLink<T> {
    /// Returns `None` when the pointer is dangling and there is no allocated `RcBox`
    #[inline]
    fn inner(&self) -> Option<WeakRcInner<'_>> {
        if is_dangling(self.ptr.as_ptr()) {
            None
        }
        else {
            // We are careful to *not* create a reference covering the "data" field, as
            // the field may be mutated concurrently (for example, if the last `Rc`
            // is dropped, the data field will be dropped in-place).
            Some(unsafe {
                let ptr = self.ptr.as_ptr();
                WeakRcInner {
                    strong: &(*ptr).strong,
                    weak: &self.weak,
                }
            })
        }
    }
}

/// Allows to upgrade a weak type into a strong type
pub trait Upgradable<T> {
    /// The type `upgrade` converts to
    type Strong: Downgradable<T>;

    /// Returns true if the reference is valid so that `upgrade` will succeed
    fn can_upgrade(&self) -> bool;

    /// Tries to upgrade the reference into a `Self::Strong`
    ///
    /// This can fail if the underlying object was destroyed, in which case `None` is returned.
    fn upgrade(&self) -> Option<Self::Strong>;
}

/// Allow to downgrade a strong type into a weak type
pub trait Downgradable<T> {
    /// The type `downgrade_asyn` converts to
    type Weak: Upgradable<T>;

    /// Downgrades into a `WeakRc` that can be stored somewhere
    fn downgrade_store(self) -> WeakRc<T>;
    /// Downgrades into a weak reference that can be kept across an async call
    fn downgrade_asyn(self) -> Self::Weak;
}

/// A reference that is not weak (strong or temporary)
pub trait NonWeak<T>: Deref<Target = T> + Upgradable<T> + Downgradable<T> + Clone {
    /// Gets the number of weak (`WeakRc`) pointers to this allocation
    fn weak_count(this: &Self) -> usize;

    /// Gets the number of strong (`StrongRc` and `TempRc`) pointers to this allocation
    fn strong_count(this: &Self) -> usize;

    /// Returns true if the underlying pointers of `this` and `other` are equal
    fn ptr_eq(this: &Self, other: &Self) -> bool;

    /// Provides a raw pointer to the data
    fn as_ptr(this: &Self) -> *const T;
}

/// A weak reference that can be held across async calls
///
/// The `WeakRc` type can only be constructed from an `StrongRc`/`TempRc` and can, in contrast
/// to `TempRc` be hold across async calls. Afterwards, it can be upgraded into an `TempRc`, if
/// the object has not been destroyed in the meantime.
pub struct WeakRc<T> {
    link: NonNull<WeakRcLink<T>>,
}

impl<T> WeakRc<T> {
    /// Returns `None` when all strong references are gone.
    #[inline]
    fn strong_inner(&self) -> Option<WeakRcInner<'_>> {
        if is_dangling(self.link.as_ptr()) {
            None
        }
        else {
            unsafe { (*self.link.as_ptr()).inner() }
        }
    }

    /// Returns `None` if this weak is dangling (was never attached to an `StrongRc`)
    #[inline]
    fn weak_inner(&self) -> Option<&WeakRcLink<T>> {
        if is_dangling(self.link.as_ptr()) {
            None
        }
        else {
            unsafe { Some(&(*self.link.as_ptr())) }
        }
    }
}

impl<T> Upgradable<T> for WeakRc<T> {
    type Strong = TempRc<T>;

    fn can_upgrade(&self) -> bool {
        match self.strong_inner() {
            Some(inner) => inner.strong() > 0,
            None => false,
        }
    }

    fn upgrade(&self) -> Option<Self::Strong> {
        let inner = self.strong_inner()?;

        if inner.strong() == 0 {
            None
        }
        else {
            unsafe {
                inner.inc_strong();
                Some(TempRc::new(StrongRc {
                    ptr: (*self.link.as_ptr()).ptr,
                    phantom: PhantomData,
                }))
            }
        }
    }
}

impl<T> Default for WeakRc<T> {
    #[inline(always)]
    fn default() -> Self {
        Self { link: dangling() }
    }
}

impl<T> Clone for WeakRc<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        if let Some(inner) = self.weak_inner() {
            inner.inc_weak();
        }
        Self { link: self.link }
    }
}

impl<T> Drop for WeakRc<T> {
    fn drop(&mut self) {
        let Some(inner) = self.weak_inner()
        else {
            return;
        };

        inner.dec_weak();
        if inner.weak() == 0 {
            unsafe {
                drop(Box::from_raw(self.link.as_ptr()));
            }
        }
    }
}

/// A strong reference
///
/// In contrast to `TempRc`, `StrongRc` can be held across async calls and should therefore not be
/// used for temporary access to the internal object, but for storing the object.
pub struct StrongRc<T> {
    ptr: NonNull<RcBox<T>>,
    phantom: PhantomData<RcBox<T>>,
}

impl<T> StrongRc<T> {
    // Creates a new `StrongRc` from given object
    pub fn new(value: T) -> Self {
        unsafe {
            let rcbox = NonNull::new_unchecked(Box::into_raw(Box::new(RcBox {
                strong: Cell::new(1),
                weak_link: NonNull::dangling(),
                value,
            })));

            // There is an implicit weak pointer owned by all the strong pointers, which ensures
            // that the weak destructor never frees the allocation while the strong destructor is
            // running, even if the weak pointer is stored inside the strong one.
            (*rcbox.as_ptr()).weak_link =
                NonNull::new_unchecked(Box::into_raw(Box::new(WeakRcLink {
                    ptr: rcbox,
                    weak: Cell::new(1),
                })));

            Self {
                ptr: rcbox,
                phantom: PhantomData,
            }
        }
    }

    #[inline(always)]
    fn inner(&self) -> &RcBox<T> {
        // This unsafety is ok because while this Rc is alive we're guaranteed
        // that the inner pointer is valid.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Downgradable<T> for StrongRc<T> {
    type Weak = StrongRc<T>;

    #[inline(always)]
    fn downgrade_store(self) -> WeakRc<T> {
        self.inner().inc_weak();
        // Make sure we do not create a dangling Weak
        debug_assert!(!is_dangling(self.ptr.as_ptr()));
        // count will be decreased in drop of self
        WeakRc {
            link: self.inner().weak_link,
        }
    }

    #[inline(always)]
    fn downgrade_asyn(self) -> Self::Weak {
        self
    }
}

impl<T> Upgradable<T> for StrongRc<T> {
    type Strong = Self;

    #[inline(always)]
    fn can_upgrade(&self) -> bool {
        true
    }

    #[inline(always)]
    fn upgrade(&self) -> Option<Self> {
        Some(self.clone())
    }
}

impl<T> Clone for StrongRc<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        self.inner().inc_strong();
        Self {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T> Drop for StrongRc<T> {
    fn drop(&mut self) {
        unsafe {
            self.inner().dec_strong();
            if self.inner().strong() == 0 {
                let weak = &mut *self.ptr.as_mut().weak_link.as_mut();

                // invalidate back link to us in WeakRcLink
                weak.ptr = dangling();
                // now that all strong references are gone, remove the additional weak reference to
                // destroy the WeakRcLink as soon as all weak refs are gone as well.
                weak.dec_weak();

                // destroy WeakRcLink if there are no references anymore
                if weak.weak() == 0 {
                    drop(Box::from_raw(self.inner().weak_link.as_ptr()));
                }

                // destroy RcBox
                drop(Box::from_raw(self.ptr.as_ptr()));
            }
        }
    }
}

impl<T: fmt::Display> fmt::Display for StrongRc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug> fmt::Debug for StrongRc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> Deref for StrongRc<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.inner().value
    }
}

impl<T> NonWeak<T> for StrongRc<T> {
    #[inline(always)]
    fn weak_count(this: &Self) -> usize {
        this.inner().weak() - 1
    }

    #[inline(always)]
    fn strong_count(this: &Self) -> usize {
        this.inner().strong()
    }

    #[inline(always)]
    fn ptr_eq(this: &Self, other: &Self) -> bool {
        ptr::addr_eq(this.ptr.as_ptr(), other.ptr.as_ptr())
    }

    #[inline(always)]
    fn as_ptr(this: &Self) -> *const T {
        let ptr: *mut RcBox<T> = NonNull::as_ptr(this.ptr);

        // SAFETY: This cannot go through Deref::deref or Rc::inner because
        // this is required to retain raw/mut provenance such that e.g. `get_mut` can
        // write through the pointer after the Rc is recovered through `from_raw`.
        unsafe { ptr::addr_of_mut!((*ptr).value) }
    }
}

/// A temporary reference for in-between async calls
///
/// An `TempRc` provides access to an underlying object of type `T`, but holding an `TempRc` does
/// not allow async calls (that is, `crate::wait_for` will panic). Performing an async call
/// requires to either drop all `TempRc`s or convert them into `WeakRc`s. The latter allows to
/// upgrade them back into `TempRc`s after the async call, but in a checked way. That is, if the
/// object was destroyed in the meantime, the upgrade will fail.
pub struct TempRc<T> {
    inner: StrongRc<T>,
}

impl<T> TempRc<T> {
    // Creates a new `TempRc` from given strong reference
    #[inline(always)]
    pub fn new(value: StrongRc<T>) -> Self {
        inc_temp_refs();
        Self { inner: value }
    }

    /// Turns this `TempRc` into a `StrongRc`
    ///
    /// # Safety
    ///
    /// In contrast to `into_strong`, this method turns it into a `StrongRc` regardless of whether
    /// there are other references. The `StrongRc` can be held across async calls, preventing that
    /// the inner object can be destructed.
    pub unsafe fn into_strong_unchecked(this: Self) -> StrongRc<T> {
        dec_temp_refs();
        let man = ManuallyDrop::new(this);
        // safety: we don't access `man` afterwards
        unsafe { ptr::read(&man.inner) }
    }

    /// Turns this `TempRc` into a `StrongRc` if there is only one reference
    ///
    /// With only one reference, we own the internal object, implying that no one else has a
    /// reference that allows him to take it away from us. Therefore, we can also turn it into a
    /// `StrongRc`.
    ///
    /// If there is just one reference, `Ok(StrongRc)` is returned. Otherwise `Err(Self)` is
    /// returned.
    pub fn into_strong(this: Self) -> Result<StrongRc<T>, Self> {
        if StrongRc::strong_count(&this.inner) == 1 {
            // safety: we have only one reference, so it's safe to turn this into a StrongRc
            Ok(unsafe { Self::into_strong_unchecked(this) })
        }
        else {
            Err(this)
        }
    }
}

impl<T> Downgradable<T> for TempRc<T> {
    type Weak = WeakRc<T>;

    #[inline(always)]
    fn downgrade_store(self) -> WeakRc<T> {
        self.inner.clone().downgrade_store()
    }

    #[inline(always)]
    fn downgrade_asyn(self) -> Self::Weak {
        self.downgrade_store()
    }
}

impl<T> Upgradable<T> for TempRc<T> {
    type Strong = TempRc<T>;

    fn can_upgrade(&self) -> bool {
        true
    }

    fn upgrade(&self) -> Option<Self::Strong> {
        Some(self.clone())
    }
}

impl<T> Clone for TempRc<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self::new(self.inner.clone())
    }
}

impl<T> Drop for TempRc<T> {
    #[inline(always)]
    fn drop(&mut self) {
        dec_temp_refs();
    }
}

impl<T: fmt::Display> fmt::Display for TempRc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug> fmt::Debug for TempRc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> Deref for TempRc<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<T> NonWeak<T> for TempRc<T> {
    #[inline(always)]
    fn weak_count(this: &Self) -> usize {
        StrongRc::weak_count(&this.inner)
    }

    /// Gets the number of strong (`StrongRc` and `TempRc`) pointers to this allocation
    fn strong_count(this: &Self) -> usize {
        StrongRc::strong_count(&this.inner)
    }

    #[inline(always)]
    fn ptr_eq(this: &Self, other: &Self) -> bool {
        StrongRc::ptr_eq(&this.inner, &other.inner)
    }

    #[inline(always)]
    fn as_ptr(this: &Self) -> *const T {
        StrongRc::as_ptr(&this.inner)
    }
}

/// A lock for async calls
///
/// Holding an instance of `AsyncLock` prevents that async calls are done.
pub struct AsyncLock;

impl AsyncLock {
    /// Creates a new `AsyncLock`
    pub fn new() -> Self {
        inc_temp_refs();
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
        dec_temp_refs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DropMarker<'a>(&'a Cell<bool>);
    impl<'a> DropMarker<'a> {
        fn new(r: &'a Cell<bool>) -> Self {
            r.set(false);
            Self(r)
        }
    }
    impl<'a> Drop for DropMarker<'a> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    #[test]
    fn single_strong() {
        let dropped = Cell::from(false);
        let rc = StrongRc::new(DropMarker::new(&dropped));
        assert_eq!(StrongRc::strong_count(&rc), 1);
        assert_eq!(StrongRc::weak_count(&rc), 0);
        assert_eq!(dropped.get(), false);
        drop(rc);
        assert_eq!(dropped.get(), true);
    }

    #[test]
    fn multiple_strong() {
        let dropped = Cell::from(false);
        let rc = StrongRc::new(DropMarker::new(&dropped));
        let clone = rc.clone();
        assert_eq!(StrongRc::strong_count(&rc), 2);
        assert_eq!(StrongRc::weak_count(&rc), 0);
        assert_eq!(dropped.get(), false);
        drop(rc);
        assert_eq!(StrongRc::strong_count(&clone), 1);
        assert_eq!(dropped.get(), false);
        drop(clone);
        assert_eq!(dropped.get(), true);
    }

    #[test]
    fn strong_then_weak() {
        let dropped = Cell::from(false);
        let rc = StrongRc::new(DropMarker::new(&dropped));
        let weak = rc.clone().downgrade_store();
        assert_eq!(StrongRc::strong_count(&rc), 1);
        assert_eq!(StrongRc::weak_count(&rc), 1);
        assert_eq!(weak.can_upgrade(), true);
        assert_eq!(dropped.get(), false);
        drop(rc);
        assert_eq!(weak.can_upgrade(), false);
        assert_eq!(dropped.get(), true);
    }

    #[test]
    fn weak_then_strong() {
        let dropped = Cell::from(false);
        let rc = StrongRc::new(DropMarker::new(&dropped));
        let weak = rc.clone().downgrade_store();
        assert_eq!(StrongRc::strong_count(&rc), 1);
        assert_eq!(StrongRc::weak_count(&rc), 1);
        assert_eq!(weak.can_upgrade(), true);
        assert_eq!(dropped.get(), false);
        drop(weak);
        assert_eq!(StrongRc::strong_count(&rc), 1);
        assert_eq!(StrongRc::weak_count(&rc), 0);
        assert_eq!(dropped.get(), false);
        drop(rc);
        assert_eq!(dropped.get(), true);
    }

    #[test]
    fn invalid_weak() {
        let weak: WeakRc<u64> = WeakRc::default();
        assert_eq!(weak.can_upgrade(), false);
        assert!(weak.upgrade().is_none());
        drop(weak);
    }
}
