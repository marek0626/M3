/*
 * Copyright (C) 2021 Nils Asmussen, Barkhausen Institut
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

use core::intrinsics;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::cell::{StaticCell, StaticUnsafeCell};
use crate::mem;

pub const MAX_MSG_SIZE: usize = 512;

static DEF_MSG_BUF: StaticUnsafeCell<MsgBuf> = StaticUnsafeCell::new(MsgBuf {
    bytes: [mem::MaybeUninit::new(0); MAX_MSG_SIZE],
    pos: 0,
});
static DEF_MSG_USED: StaticCell<bool> = StaticCell::new(false);

/// A reference to a `MsgBuf` that makes sure that each `MsgBuf` is used at most once at a time.
pub struct MsgBufRef {
    buf: NonNull<MsgBuf>,
}

impl MsgBufRef {
    fn new(buf: NonNull<MsgBuf>) -> Self {
        assert!(!DEF_MSG_USED.get());
        DEF_MSG_USED.set(true);
        Self { buf }
    }
}

impl Drop for MsgBufRef {
    fn drop(&mut self) {
        // safety: we make sure that no one else can hold a reference to this buffer
        unsafe {
            (*self.buf.as_ptr()).pos = 0;
        }
        DEF_MSG_USED.set(false);
    }
}

impl Deref for MsgBufRef {
    type Target = MsgBuf;

    fn deref(&self) -> &Self::Target {
        // safety: we make sure that no one else can hold a reference to this buffer
        unsafe { &*self.buf.as_ptr() }
    }
}

impl DerefMut for MsgBufRef {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // safety: we make sure that no one else can hold a reference to this buffer
        unsafe { &mut *self.buf.as_ptr() }
    }
}

// messages cannot contain a page boundary, so make sure that they are max-size-aligned
#[repr(C, align(512))]
/// A buffer for messages that takes care of proper alignment to fulfill the alignment requirements
/// of the TCU.
pub struct MsgBuf {
    bytes: [mem::MaybeUninit<u8>; MAX_MSG_SIZE],
    pos: usize,
}

impl MsgBuf {
    /// Borrows the default message buffer
    ///
    /// Every message buffer can only be used once at a time, so that the caller has to make sure
    /// that the returned `MsgBufRef` is dropped before the next call to `borrow_ref`.
    /// Alternatively, `MsgBuf::new` can be used to allocate a new buffer.
    pub fn borrow_def() -> MsgBufRef {
        // safety: MsgBufRef takes care that there is no other user of DEF_MSG_BUF
        MsgBufRef::new(unsafe { NonNull::new_unchecked(DEF_MSG_BUF.as_ptr()) })
    }

    /// Creates a new zero'd message buffer containing an empty message
    pub const fn new_initialized() -> Self {
        Self {
            bytes: [mem::MaybeUninit::new(0); MAX_MSG_SIZE],
            pos: 0,
        }
    }

    /// Creates a new message buffer containing an empty message
    pub fn new() -> Self {
        Self {
            bytes: unsafe { mem::MaybeUninit::uninit().assume_init() },
            pos: 0,
        }
    }

    /// Returns the message bytes
    pub fn bytes(&self) -> &[u8] {
        // safety: 0..`pos` is always initialized
        unsafe { intrinsics::transmute(&self.bytes[0..self.pos]) }
    }

    /// Returns the number of bytes to send
    pub fn size(&self) -> usize {
        self.pos
    }

    /// Returns a mutable u64 slice to the message bytes
    ///
    /// # Safety
    ///
    /// The caller cannot read the words since they are not necessarily initialized
    pub unsafe fn words_mut(&mut self) -> &mut [u64] {
        let slice = [self.bytes.as_ptr() as usize, MAX_MSG_SIZE / 8];
        intrinsics::transmute(slice)
    }

    /// Sets the number of bytes that will be sent by the TCU.
    ///
    /// # Safety
    ///
    /// The caller has to guarantee that the bytes from 0 to `pos` are initialized
    pub unsafe fn set_size(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Sets the message to the given slice
    pub fn set_from_slice(&mut self, bytes: &[u8]) {
        self.bytes[0..bytes.len()].write_copy_of_slice(bytes);
        self.pos = bytes.len();
    }
}

impl Clone for MsgBuf {
    fn clone(&self) -> Self {
        let mut copy = Self::new();
        copy.bytes[0..self.pos].write_copy_of_slice(self.bytes());
        copy.pos = self.pos;
        copy
    }
}

impl Default for MsgBuf {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(align(4096))]
/// A buffer that is page aligned in order to maximize performance of TCU transfers.
pub struct AlignedBuf<const N: usize> {
    data: [u8; N],
}

impl<const N: usize> AlignedBuf<N> {
    /// Creates a new `AlignedBuf` filled with zeros
    pub const fn new_zeroed() -> Self {
        Self { data: [0u8; N] }
    }
}

impl<const N: usize> Deref for AlignedBuf<N> {
    type Target = [u8; N];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<const N: usize> DerefMut for AlignedBuf<N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut buf = MsgBuf::default();
        assert_eq!(buf.bytes().len(), 0);
        assert_eq!(buf.size(), 0);

        buf.set_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(buf.size(), 8);

        let clone = buf.clone();
        assert_eq!(clone.size(), buf.size());
        assert_eq!(clone.bytes(), buf.bytes());
    }

    #[test]
    #[should_panic]
    fn double_use() {
        let mut buf = MsgBuf::borrow_def();
        assert_eq!(buf.size(), 0);
        buf.set_from_slice(&[0, 1]);
        assert_eq!(buf.size(), 2);

        // panics as the default buffer is already in use
        let _buf2 = MsgBuf::borrow_def();
    }
}
