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

use base::boxed::Box;
use base::cell::RefCell;
use base::col::String;
use base::errors::{Code, Error};
use base::mem::MsgBuf;
use base::rc::Rc;
use base::tcu;
use core::fmt;

use crate::cap::{KObjectOwnedRef, KObjectWeakRef, RGateObject};
use crate::com::{QueueId, SendQueue};
use crate::tiles::Activity;

pub struct Service {
    act: KObjectWeakRef<Activity>,
    name: String,
    rgate: KObjectWeakRef<RGateObject>,
    queue: RefCell<Box<SendQueue>>,
}

impl Service {
    pub fn new(
        act: KObjectOwnedRef<Activity>,
        name: String,
        rgate: KObjectOwnedRef<RGateObject>,
    ) -> Rc<Self> {
        Rc::new(Service {
            name,
            rgate: rgate.downgrade(),
            queue: RefCell::from(SendQueue::new(QueueId::Serv(act.id()), act.tile_id())),
            act: act.downgrade(),
        })
    }

    pub fn activity(&self) -> KObjectOwnedRef<Activity> {
        self.act.upgrade().unwrap()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn send(&self, lbl: tcu::Label, msg: &MsgBuf) -> Result<thread::Event, Error> {
        let rg = self
            .rgate
            .upgrade()
            .ok_or_else(|| Error::new(Code::NotFound))?;
        let (_, rep) = rg.location().unwrap();
        self.queue.borrow_mut().send(rep, lbl, msg)
    }

    pub fn abort(&self) {
        self.queue.borrow_mut().abort();
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
