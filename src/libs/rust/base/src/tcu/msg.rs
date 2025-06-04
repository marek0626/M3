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

//! The TCU's message types

use core::marker::PhantomData;
use core::ops::Deref;
use core::ptr::{slice_from_raw_parts, NonNull};
use core::slice;

use crate::errors::Error;
use crate::mem::{self, VirtAddr};

use crate::tcu::{EpId, Label, TCU};

/// The TCU header
#[repr(C, packed)]
#[derive(Copy, Clone, Default, Debug)]
pub struct Header {
    other: u32,
    sender_ep: u16,
    reply_ep: u16,
    reply_label: Label,
    label: Label,
    #[cfg(M3_TARGET = "hw23")]
    _pad: u64,
    #[cfg(any(M3_TARGET = "hw", M3_TARGET = "gem5"))]
    sgen: u16,
    #[cfg(any(M3_TARGET = "hw", M3_TARGET = "gem5"))]
    rgen: u16,
    #[cfg(any(M3_TARGET = "hw", M3_TARGET = "gem5"))]
    _pad: u32,
}

impl Header {
    /// Returns the length of the message payload in bytes
    pub fn length(&self) -> usize {
        (self.other >> 19) as usize & ((1 << 13) - 1)
    }

    /// Returns the label that has been assigned to the sender of the message
    pub fn label(&self) -> Label {
        self.label
    }
}

/// The TCU message consisting of the header and the payload
#[repr(C, align(8))]
#[derive(Debug)]
pub struct Message {
    pub header: Header,
    pub data: [u8],
}

impl Message {
    /// Returns the message data as a slice of u64's
    pub fn as_words(&self) -> &[u64] {
        // safety: we trust the TCU
        unsafe {
            let ptr = self.data.as_ptr() as *const u64;
            slice::from_raw_parts(ptr, self.header.length() / 8)
        }
    }
}

/// Received message that owns a message slot
///
/// The message is read-only. [`Self`] owns the slot to prevent answering the message while a
/// reference to the contents are held. This avoids data races between the Rust code and the TCU
/// (would be UB).
#[derive(Default)]
pub struct OwnedMessage {
    rep: EpId,
    msg: Option<NonNull<()>>,
    off: usize,
    /// Signal the compiler that we own a message
    _phantom: PhantomData<Message>,
}

impl OwnedMessage {
    const GONE: &'static str = "message already gone";

    /// Creates a new instance with given message
    ///
    /// # Safety
    ///
    /// Safe if rep, base, and off describe a valid message for as long as `Self::msg` is [`Some`],
    /// i.e., the message is not answered.
    pub unsafe fn new(rep: EpId, base: VirtAddr, off: usize) -> Self {
        Self {
            rep,
            msg: Some(NonNull::new((base.as_local() + off) as *mut ()).unwrap()),
            off,
            _phantom: Default::default(),
        }
    }

    /// Invalidates the internal message, so that is no longer accessible
    pub fn invalidate(&mut self) {
        self.msg = None;
    }

    /// Acknowledge the message
    ///
    /// Afterwards, `self` should not be interacted with anymore.
    pub fn ack(&mut self) {
        self.take();
        // SAFETY: If there is some message in self, it is valid.
        TCU::ack_msg(self.rep, self.off).unwrap();
    }

    /// Send a reply
    ///
    /// Afterwards, `self` should not be interacted with anymore.
    pub fn reply(&mut self, reply: &mem::MsgBuf) -> Result<(), Error> {
        self.take();
        // SAFETY: If there is some message in self, it is valid.
        TCU::reply(self.rep, reply, self.off)
        // Self is dropped and the user cannot access the message anymore.
    }

    /// Take message **before invalidating** it
    ///
    /// This leaves no dangling pointer behind.
    fn take(&mut self) {
        self.msg.take().expect(Self::GONE);
    }
}

impl Deref for OwnedMessage {
    type Target = Message;

    fn deref(&self) -> &Self::Target {
        let msg = self.msg.expect(Self::GONE);
        // SAFETY: If there is some message in self, it is valid.
        unsafe {
            let header = msg.cast::<Header>();
            let length = header.as_ref().length();
            // Add length information to pointer.
            let msg: *const [()] = slice_from_raw_parts(msg.as_ptr(), length);
            &*(msg as *const Self::Target)
        }
        // TODO: Prevent the user from (safely) acknowledging the returned
        // message.
    }
}

impl Drop for OwnedMessage {
    /// # Panics
    /// Panics if the message is not answered yet.
    /// This catches errors of unhandled messages.
    fn drop(&mut self) {
        if self.msg.is_some() {
            panic!("unanswered message dropped");
        }
    }
}
