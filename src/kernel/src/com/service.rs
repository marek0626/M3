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

use anyhow::anyhow;

use base::boxed::Box;
use base::cell::RefCell;
use base::col::String;
use base::errors::{Code, Error};
use base::mem::MsgBuf;
use base::rc::Rc;
use base::{format, tcu};
use core::fmt;

use thread::{Downgradable, TempRc, Upgradable, WeakRc};

use crate::cap::RGateObject;
use crate::com::{QueueId, SendQueue};
use crate::tiles::Activity;

pub struct Service {
    act: WeakRc<Activity>,
    name: String,
    rgate: WeakRc<RGateObject>,
    queue: RefCell<Box<SendQueue>>,
    // note that we deliberately allow just one derive at a time here, because otherwise it's
    // tricky to prevent memory DOS attacks on the kernel: since we cannot force the server to use
    // the derive_srv_fin syscall, it could just reply to the service request and thereby allow
    // applications to start another derive_srv without really finishing the old one. If we don't
    // want to keep a thread around to handle the reply to the service request (where we could
    // finish the current derive_srv_req before issuing a new one), the only other way is probably
    // to check specifically for this reply in SendQueue::received_reply and finish it there, which
    // does not seem worth it.
    cur_derive: RefCell<Option<WeakRc<Activity>>>,
}

impl Service {
    pub fn new(act: TempRc<Activity>, name: String, rgate: TempRc<RGateObject>) -> Rc<Self> {
        Rc::new(Service {
            name,
            rgate: rgate.downgrade_store(),
            queue: RefCell::from(SendQueue::new(QueueId::Serv, act.tile_id())),
            act: act.downgrade_store(),
            cur_derive: RefCell::from(None),
        })
    }

    pub fn activity(&self) -> TempRc<Activity> {
        self.act.upgrade().unwrap()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_derive_act(&self, act: TempRc<Activity>) -> anyhow::Result<()> {
        let mut cur_derive = self.cur_derive.borrow_mut();
        if cur_derive.is_some() {
            return Err(anyhow!(Error::new(Code::Exists))
                .context(format!("Derive in progress for activity {}", act.id())));
        }
        *cur_derive = Some(act.downgrade_store());
        Ok(())
    }

    pub fn fetch_derive_act(&self) -> anyhow::Result<TempRc<Activity>> {
        let act = self
            .cur_derive
            .borrow_mut()
            .take()
            .ok_or_else(|| anyhow!(Error::new(Code::NotFound)).context("No derive found"))?;
        act.upgrade()
            .ok_or_else(|| anyhow!(Error::new(Code::ObjectGone)).context("Activity was destroyed"))
    }

    pub fn send(&self, lbl: tcu::Label, msg: &MsgBuf) -> anyhow::Result<thread::Event> {
        let rg = self.rgate.upgrade().ok_or_else(|| {
            anyhow!(Error::new(Code::ObjectGone)).context("RecvGate was destroyed")
        })?;
        let (_, rep) = rg.location().ok_or_else(|| {
            anyhow!(Error::new(Code::RecvGone)).context("RecvGate was deactivated")
        })?;
        self.queue.borrow_mut().send(rep, lbl, msg)
    }

    pub fn abort(&self) {
        self.queue.borrow_mut().abort();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // if there is still an open derive, notify the activity via upcall
        if let Some(act_weak) = self.cur_derive.borrow_mut().take() {
            if let Some(act) = act_weak.upgrade() {
                if let Some(derive) = act.finish_derive() {
                    act.upcall_derive_srv(derive.event, Code::ObjectGone);
                }
            }
        }
    }
}

impl fmt::Debug for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Service[name={}, rgate=", self.name)?;
        if let Some(rg) = self.rgate.upgrade() {
            rg.print_loc(f)?;
        }
        else {
            write!(f, "?")?;
        }
        write!(f, "]")
    }
}
