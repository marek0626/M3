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
use base::build_vmsg;
use base::cell::{Cell, RefCell, StaticRefCell};
use base::col::{String, ToString, Vec};
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::{self, CapRngDesc, CapSel, CapType, TileDesc};
use base::log;
use base::mem::{MsgBuf, PhysAddr, PhysAddrRaw, VirtAddr};
use base::rc::Rc;
use base::tcu::Label;
use base::tcu::{ActId, EpId, TileId, STD_EPS_COUNT, UPCALL_REP_OFF};
use bitflags::bitflags;
use core::fmt;

use crate::cap::{
    wait_for_async, CapTable, Capability, EPObject, KMemObject, KObject, KObjectOwnedRef,
    KObjectWeakRef, TileObject,
};
use crate::com::{QueueId, SendQueue};
use crate::ktcu;
use crate::platform;
use crate::thread_startup_async;
use crate::tiles::{loader, tilemng, ActivityMng};

bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct ActivityFlags : u32 {
        const IS_ROOT     = 1;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    INIT,
    RUNNING,
    DEAD,
}

struct ExitWait {
    id: ActId,
    event: u64,
    sels: Vec<u64>,
}

pub const KERNEL_ID: ActId = 0xFFFF;
pub const INVAL_ID: ActId = 0xFFFF;

static EXIT_EVENT: Code = Code::Success;
static EXIT_LISTENERS: StaticRefCell<Vec<ExitWait>> = StaticRefCell::new(Vec::new());

pub struct Activity {
    id: ActId,
    name: String,
    flags: ActivityFlags,
    eps_start: EpId,
    // keep a copy of the tile id for performance reasons (does never change)
    tile_id: TileId,

    tile: KObjectWeakRef<TileObject>,
    // we currently have to store a strong reference here, because the activity needs access to it
    // until it is fully destructed to give back all the kmem quota it uses for its capabilities
    kmem: Rc<KMemObject>,

    state: Cell<State>,
    exit_code: Cell<Option<Code>>,
    first_sel: Cell<CapSel>,

    obj_caps: RefCell<CapTable>,
    map_caps: RefCell<CapTable>,

    eps: RefCell<Vec<KObjectWeakRef<EPObject>>>,
    rbuf_phys: Cell<PhysAddr>,
    upcalls: RefCell<Box<SendQueue>>,
}

impl Activity {
    pub fn new(
        name: &str,
        id: ActId,
        tile: KObjectOwnedRef<TileObject>,
        eps_start: EpId,
        kmem: KObjectOwnedRef<KMemObject>,
        flags: ActivityFlags,
    ) -> Result<Rc<Self>, Error> {
        let act = Rc::new(Activity {
            id,
            name: name.to_string(),
            flags,
            eps_start,
            tile_id: tile.tile(),
            kmem: kmem.inner().clone(),
            state: Cell::from(State::INIT),
            exit_code: Cell::from(None),
            first_sel: Cell::from(kif::FIRST_FREE_SEL),
            obj_caps: RefCell::from(CapTable::default()),
            map_caps: RefCell::from(CapTable::default()),
            eps: RefCell::from(Vec::new()),
            rbuf_phys: Cell::from(PhysAddr::default()),
            upcalls: RefCell::from(SendQueue::new(QueueId::Activity(id), tile.tile())),
            tile: tile.downgrade(),
        });

        {
            act.obj_caps.borrow_mut().set_activity(&act);
            act.map_caps.borrow_mut().set_activity(&act);

            let tile = act.tile.upgrade().unwrap();

            // kmem cap
            act.obj_caps().borrow_mut().insert(Capability::new(
                kif::SEL_KMEM,
                KObject::KMem(act.kmem.clone()),
            ))?;
            // tile cap
            act.obj_caps().borrow_mut().insert(Capability::new(
                kif::SEL_TILE,
                KObject::Tile(tile.inner().clone()),
            ))?;
            // cap for own activity
            act.obj_caps().borrow_mut().insert(Capability::new(
                kif::SEL_ACT,
                KObject::Activity(act.clone()),
            ))?;

            // alloc standard EPs
            tilemng::tilemux(act.tile_id()).alloc_eps(eps_start, STD_EPS_COUNT);
            tile.alloc(STD_EPS_COUNT);

            // add us to tile
            tile.add_activity();
        }

        // some system calls are blocking, leading to a thread switch in the kernel. there is just
        // one syscall per activity at a time, thus at most one additional thread per activity is required.
        thread::add_thread(VirtAddr::from(thread_startup_async as *const ()), 0);

        Ok(act)
    }

    pub fn init_async(act: KObjectOwnedRef<Self>) -> Result<(), Error> {
        use base::kif::PageFlags;

        let act_weak = act.clone().downgrade();

        loader::init_activity_async(act)?;

        let act = act_weak
            .upgrade()
            .ok_or_else(|| Error::new(Code::NotFound))?;

        let desc = platform::tile_desc(act.tile_id());
        if !desc.is_device() {
            // get physical address of receive buffer
            let rbuf_virt = desc.rbuf_std_space().0;
            let (act, rbuf_phys) = if desc.has_virtmem() {
                let act_id = act.id();
                let tile_id = act.tile_id();
                let act_weak = act.downgrade();

                let glob = crate::tiles::TileMux::translate_async(
                    tilemng::tilemux(tile_id),
                    act_id,
                    rbuf_virt,
                    PageFlags::RW,
                )?;

                let act = act_weak
                    .upgrade()
                    .ok_or_else(|| Error::new(Code::NotFound))?;
                let phys = ktcu::glob_to_phys_remote(act.tile_id(), glob, base::kif::PageFlags::RW)
                    .unwrap();
                (act, phys)
            }
            else {
                (act, rbuf_virt.as_phys(desc))
            };

            act.init_eps(rbuf_phys)
        }
        else {
            Ok(())
        }
    }

    pub fn init_eps(&self, rbuf_phys: PhysAddr) -> Result<(), Error> {
        use crate::cap::{RGateObject, SGateObject};
        use base::cfg;
        use base::tcu;

        let act = if platform::is_shared(self.tile_id()) {
            self.id()
        }
        else {
            INVAL_ID
        };

        self.rbuf_phys.set(rbuf_phys);

        let mut tilemux = tilemng::tilemux(self.tile_id());

        // attach syscall send endpoint
        {
            let rgate = KObjectOwnedRef::new(RGateObject::new(
                cfg::SYSC_RBUF_ORD,
                cfg::SYSC_RBUF_ORD,
                false,
            ));
            rgate.activate(
                platform::kernel_tile(),
                ktcu::KSYS_EP,
                PhysAddr::new_raw(platform::tile_desc(self.tile_id()), 0xDEADBEEF),
            );
            let _rg_clone = rgate.clone(); // keep one strong reference
            let sgate = KObjectOwnedRef::new(SGateObject::new(
                rgate.downgrade(),
                self.id() as tcu::Label,
                1,
            ));
            tilemux.config_snd_ep(self.eps_start + tcu::SYSC_SEP_OFF, act, &sgate)?;
        }

        // attach syscall receive endpoint
        let mut rbuf_addr = self.rbuf_phys.get();
        {
            let rgate = KObjectOwnedRef::new(RGateObject::new(
                cfg::SYSC_RBUF_ORD,
                cfg::SYSC_RBUF_ORD,
                false,
            ));
            rgate.activate(
                self.tile_id(),
                self.eps_start + tcu::SYSC_REP_OFF,
                rbuf_addr,
            );
            tilemux.config_rcv_ep(self.eps_start + tcu::SYSC_REP_OFF, act, None, &rgate)?;
            rbuf_addr += cfg::SYSC_RBUF_SIZE as PhysAddrRaw;
        }

        // attach upcall receive endpoint
        {
            let rgate = KObjectOwnedRef::new(RGateObject::new(
                cfg::UPCALL_RBUF_ORD,
                cfg::UPCALL_RBUF_ORD,
                false,
            ));
            rgate.activate(
                self.tile_id(),
                self.eps_start + tcu::UPCALL_REP_OFF,
                rbuf_addr,
            );
            tilemux.config_rcv_ep(
                self.eps_start + tcu::UPCALL_REP_OFF,
                act,
                Some(self.eps_start + tcu::UPCALL_RPLEP_OFF),
                &rgate,
            )?;
            rbuf_addr += cfg::UPCALL_RBUF_SIZE as PhysAddrRaw;
        }

        // attach default receive endpoint
        {
            let rgate = KObjectOwnedRef::new(RGateObject::new(
                cfg::DEF_RBUF_ORD,
                cfg::DEF_RBUF_ORD,
                false,
            ));
            rgate.activate(self.tile_id(), self.eps_start + tcu::DEF_REP_OFF, rbuf_addr);
            tilemux.config_rcv_ep(self.eps_start + tcu::DEF_REP_OFF, act, None, &rgate)?;
        }

        Ok(())
    }

    pub fn id(&self) -> ActId {
        self.id
    }

    pub fn tile(&self) -> KObjectOwnedRef<TileObject> {
        self.tile.upgrade().unwrap()
    }

    pub fn tile_weak(&self) -> &KObjectWeakRef<TileObject> {
        &self.tile
    }

    pub fn tile_id(&self) -> TileId {
        self.tile_id
    }

    pub fn tile_desc(&self) -> TileDesc {
        platform::tile_desc(self.tile_id())
    }

    pub fn kmem(&self) -> &Rc<KMemObject> {
        &self.kmem
    }

    pub fn rbuf_addr(&self) -> PhysAddr {
        self.rbuf_phys.get()
    }

    pub fn eps_start(&self) -> EpId {
        self.eps_start
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn obj_caps(&self) -> &RefCell<CapTable> {
        &self.obj_caps
    }

    pub fn map_caps(&self) -> &RefCell<CapTable> {
        &self.map_caps
    }

    pub fn state(&self) -> State {
        self.state.get()
    }

    pub fn is_root(&self) -> bool {
        self.flags.contains(ActivityFlags::IS_ROOT)
    }

    pub fn first_sel(&self) -> CapSel {
        self.first_sel.get()
    }

    pub fn set_first_sel(&self, sel: CapSel) {
        self.first_sel.set(sel);
    }

    pub fn fetch_exit_code(&self) -> Option<Code> {
        self.exit_code.replace(None)
    }

    pub fn add_ep(&self, ep: KObjectOwnedRef<EPObject>) {
        self.eps.borrow_mut().push(ep.downgrade());
    }

    pub fn rem_ep(&self, ep: &KObjectOwnedRef<EPObject>) {
        self.eps
            .borrow_mut()
            .retain(|e| e.upgrade().unwrap().ep() != ep.ep());
    }

    fn fetch_exit(&self, sels: &[u64]) -> Option<(CapSel, Code)> {
        for sel in sels {
            let wact = self
                .obj_caps()
                .borrow()
                .get(*sel as CapSel)
                // XXX
                .map(|c| c.get().get().clone());
            match wact {
                Some(KObject::Activity(wv)) => {
                    if wv.id() == self.id() {
                        continue;
                    }

                    if let Some(code) = wv.fetch_exit_code() {
                        return Some((*sel, code));
                    }
                },
                _ => continue,
            }
        }

        None
    }

    // TODO
    pub fn wait_exit_async(&self, event: u64, sels: &[u64]) -> Option<(CapSel, Code)> {
        let res = loop {
            // independent of how we notify the activity, check for exits in case the activity we wait for
            // already exited.
            if let Some((sel, code)) = self.fetch_exit(sels) {
                // if we want to be notified by upcall, do that
                if event != 0 {
                    self.upcall_activity_wait(event, sel, code);
                    // we never report the result via syscall reply, but we need Some for below.
                    break Some((kif::INVALID_SEL, Code::Success));
                }
                else {
                    break Some((sel, code));
                }
            }

            // if we want to be notified by upcall, don't wait, just stop here
            if event != 0 || self.state() != State::RUNNING {
                break None;
            }

            // wait until someone exits
            let event = &EXIT_EVENT as *const _ as thread::Event;
            wait_for_async(event);
        };

        // ensure that we are removed from the list in any case. we might have started to wait
        // earlier and are now waiting again with a different selector list.
        EXIT_LISTENERS.borrow_mut().retain(|l| l.id != self.id());
        match event {
            // sync wait
            0 => res,
            // async wait
            _ => {
                // if no one exited yet, remember us
                if !sels.is_empty() && res.is_none() {
                    EXIT_LISTENERS.borrow_mut().push(ExitWait {
                        id: self.id(),
                        event,
                        sels: sels.to_vec(),
                    });
                }
                // in any case, the syscall replies "no result"
                None
            },
        }
    }

    fn send_exit_notify() {
        // notify all that wait without upcall
        let event = &EXIT_EVENT as *const _ as thread::Event;
        thread::notify(event, None);

        // send upcalls for the others
        EXIT_LISTENERS.borrow_mut().retain(|l| {
            let act = ActivityMng::activity(l.id).unwrap();
            if let Some((sel, code)) = act.fetch_exit(&l.sels) {
                act.upcall_activity_wait(l.event, sel, code);
                // remove us from the list since a activity exited
                false
            }
            else {
                true
            }
        });
    }

    pub fn upcall_activity_wait(&self, event: u64, act_sel: CapSel, exitcode: Code) {
        let mut buf = MsgBuf::borrow_def();
        let msg = kif::upcalls::ActivityWait {
            event,
            error: Code::Success,
            act_sel,
            exitcode,
        };
        build_vmsg!(buf, kif::upcalls::Operation::ActWait, msg);

        self.send_upcall::<kif::upcalls::ActivityWait>(&buf, &msg);
    }

    pub fn upcall_derive_srv(&self, event: u64, result: Result<(), Error>) {
        let mut buf = MsgBuf::borrow_def();
        let msg = kif::upcalls::DeriveSrv {
            event,
            error: Code::from(result),
        };
        build_vmsg!(buf, kif::upcalls::Operation::DeriveSrv, msg);

        self.send_upcall::<kif::upcalls::DeriveSrv>(&buf, &msg);
    }

    fn send_upcall<M: fmt::Debug>(&self, buf: &MsgBuf, msg: &M) {
        log!(
            LogFlags::KernUpcalls,
            "Sending upcall {:?} to Activity {}",
            msg,
            self.id()
        );

        self.upcalls
            .borrow_mut()
            .send(self.eps_start + UPCALL_REP_OFF, 0, buf)
            .unwrap();
    }

    pub fn start_app_async(act: KObjectOwnedRef<Activity>) -> Result<(), Error> {
        if act.state.get() != State::INIT {
            return Ok(());
        }

        act.state.set(State::RUNNING);

        let id = act.id();
        let tile_id = act.tile_id();
        drop(act);

        ActivityMng::start_activity_async(id, tile_id)
    }

    pub fn stop_app_async(
        act: KObjectOwnedRef<Activity>,
        exit_code: Code,
        is_self: bool,
        revoker: ActId,
    ) {
        if act.state.get() == State::DEAD {
            return;
        }

        log!(
            LogFlags::KernActs,
            "Stopping Activity {} [id={}]",
            act.name(),
            act.id()
        );

        if is_self {
            Self::exit_app_async(act, exit_code, false, revoker);
        }
        else if act.state.get() == State::RUNNING {
            // devices always exit successfully
            let exit_code = if act.tile_desc().is_device() {
                Code::Success
            }
            else {
                Code::Unspecified
            };
            Self::exit_app_async(act, exit_code, true, revoker);
        }
        else {
            act.state.set(State::DEAD);
            let act_weak = act.clone().downgrade();
            ActivityMng::stop_activity_async(act, true).unwrap();

            if let Some(act) = act_weak.upgrade() {
                ktcu::drop_msgs(ktcu::KSYS_EP, act.id() as Label);
            }
        }
    }

    fn exit_app_async(act: KObjectOwnedRef<Activity>, exit_code: Code, stop: bool, revoker: ActId) {
        let mut tilemux = tilemng::tilemux(act.tile_id());
        // force-invalidate standard EPs
        for ep in act.eps_start..act.eps_start + STD_EPS_COUNT as EpId {
            // ignore failures
            tilemux.invalidate_ep(act.id(), ep, true, false).ok();
        }
        drop(tilemux);

        // force-invalidate all other EPs of this activity
        for ep in &*act.eps.borrow_mut() {
            if let Some(ep) = ep.upgrade() {
                // ignore failures here
                ep.deconfigure(true).ok();
            }
        }

        // make sure that we don't get further syscalls by this activity
        ktcu::drop_msgs(ktcu::KSYS_EP, act.id() as Label);

        act.state.set(State::DEAD);
        act.exit_code.set(Some(exit_code));

        let act_weak = act.clone().downgrade();

        Self::force_stop_async(act, stop, revoker);

        if let Some(act) = act_weak.upgrade() {
            EXIT_LISTENERS.borrow_mut().retain(|l| l.id != act.id());

            Self::send_exit_notify();

            // if it's root, there is nobody waiting for it; just remove it
            if act.is_root() {
                ActivityMng::remove_activity_async(act.id(), revoker);
            }
        }
    }

    fn revoke_caps_async(&self, revoker: ActId) {
        CapTable::revoke_all_async(&self.obj_caps, revoker);
        CapTable::revoke_all_async(&self.map_caps, revoker);
    }

    pub fn revoke_async(&self, crd: CapRngDesc, own: bool, revoker: ActId) -> Result<(), Error> {
        // we can't use borrow_mut() here, because revoke might need to use borrow as well.
        if crd.cap_type() == CapType::Object {
            CapTable::revoke_async(self.obj_caps(), crd, own, revoker)
        }
        else {
            CapTable::revoke_async(self.map_caps(), crd, own, revoker)
        }
    }

    pub fn force_stop_async(act: KObjectOwnedRef<Activity>, stop: bool, revoker: ActId) {
        let act_weak = act.clone().downgrade();

        ActivityMng::stop_activity_async(act, stop).unwrap();

        if let Some(act_ref) = act_weak.upgrade() {
            // TODO that's broken
            let act = act_ref.inner().clone();
            drop(act_ref);
            act.revoke_caps_async(revoker);
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        self.state.set(State::DEAD);

        // free standard EPs
        tilemng::tilemux(self.tile_id()).free_eps(self.eps_start, STD_EPS_COUNT);
        let tile = self.tile();
        tile.free(STD_EPS_COUNT);

        // remove us from tile
        tile.rem_activity();

        assert!(self.obj_caps.borrow().is_empty());
        assert!(self.map_caps.borrow().is_empty());

        // remove some thread from the pool as there is one activity less now
        thread::remove_thread();

        log!(
            LogFlags::KernActs,
            "Removed Activity {} [id={}, tile={}]",
            self.name(),
            self.id(),
            self.tile_id()
        );
    }
}

impl fmt::Debug for Activity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Activity[id={}, tile={}, name={}, state={:?}]",
            self.id(),
            self.tile_id(),
            self.name(),
            self.state()
        )
    }
}
