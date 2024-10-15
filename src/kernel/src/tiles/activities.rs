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
use base::col::{String, Vec};
use base::errors::{Code, Error, VerboseError};
use base::io::LogFlags;
use base::kif::{self, CapRngDesc, CapSel, CapType, TileDesc};
use base::log;
use base::mem::{MsgBuf, PhysAddr, PhysAddrRaw, VirtAddr};
use base::tcu::{ActId, EpId, TileId, STD_EPS_COUNT, UPCALL_REP_OFF};
use base::tcu::{Label, OwnedMessage};
use bitflags::bitflags;
use core::cell::Ref;
use core::fmt;

use thread::{Downgradable, NonWeak, StrongRc, TempRc, Upgradable, WeakRc};

use crate::cap::{
    CapTable, EPObject, IntoKObject, InvalidateType, KMemObject, KObject, TileObject,
};
use crate::com::{QueueId, SendQueue};
use crate::ktcu;
use crate::thread_startup_async;
use crate::tiles::{loader, tilemng, ActivityMng};
use crate::{impl_from_kobj, platform};

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
    STOPPING,
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

pub struct DeriveSrv {
    pub src_srv: CapSel,
    pub dst_srv: CapSel,
    pub dst_sgate: CapSel,
    pub event: thread::Event,
}

pub struct Activity {
    id: ActId,
    name: String,
    flags: ActivityFlags,
    eps_start: EpId,
    // keep a copy of the tile id for performance reasons (does never change)
    tile_id: TileId,
    parent: Option<(ActId, CapSel)>,

    tile: WeakRc<TileObject>,
    kmem: WeakRc<KMemObject>,

    state: Cell<State>,
    exit_code: Cell<Option<Code>>,
    first_sel: Cell<CapSel>,

    obj_caps: RefCell<CapTable>,
    map_caps: RefCell<CapTable>,

    eps: RefCell<Vec<WeakRc<EPObject>>>,
    rbuf_phys: Cell<PhysAddr>,
    upcalls: RefCell<Box<SendQueue>>,

    cur_sysc: RefCell<OwnedMessage>,
    cur_derive_srv: RefCell<Option<DeriveSrv>>,
}

impl Activity {
    pub fn new(
        name: String,
        id: ActId,
        parent: Option<(ActId, CapSel)>,
        tile: TempRc<TileObject>,
        eps_start: EpId,
        kmem: TempRc<KMemObject>,
        flags: ActivityFlags,
    ) -> Result<StrongRc<Self>, Error> {
        let act = StrongRc::new(Activity {
            id,
            name,
            flags,
            eps_start,
            parent,
            tile_id: tile.tile(),
            kmem: kmem.downgrade_store(),
            state: Cell::from(State::INIT),
            exit_code: Cell::from(None),
            first_sel: Cell::from(kif::FIRST_FREE_SEL),
            obj_caps: RefCell::from(CapTable::default()),
            map_caps: RefCell::from(CapTable::default()),
            eps: RefCell::from(Vec::new()),
            rbuf_phys: Cell::from(PhysAddr::default()),
            upcalls: RefCell::from(SendQueue::new(QueueId::Activity, tile.tile())),
            tile: tile.clone().downgrade_store(),
            cur_sysc: RefCell::from(OwnedMessage::default()),
            cur_derive_srv: RefCell::from(None),
        });

        {
            act.obj_caps.borrow_mut().set_activity(&act);
            act.map_caps.borrow_mut().set_activity(&act);

            // alloc standard EPs
            tilemng::tilemux(act.tile_id()).alloc_eps(eps_start, STD_EPS_COUNT);
            tile.alloc_eps(STD_EPS_COUNT);

            // add us to tile
            if let Some((aid, sel)) = act.parent {
                tile.add_activity(aid, sel);
            }
        }

        // some system calls are blocking, leading to a thread switch in the kernel. there is just
        // one syscall per activity at a time, thus at most one additional thread per activity is required.
        #[cfg_attr(dylint_lib = "m3_lints", allow(async_alias))]
        thread::add_thread(VirtAddr::from(thread_startup_async as *const ()), 0);

        Ok(act)
    }

    pub fn init_async(act: StrongRc<Self>) -> Result<(), Error> {
        use base::kif::PageFlags;

        loader::init_activity_async(act.clone())?;

        let desc = platform::tile_desc(act.tile_id());
        if !desc.is_device() {
            // get physical address of receive buffer
            let rbuf_virt = desc.rbuf_std_space().0;
            let rbuf_phys = if desc.has_virtmem() {
                let glob = crate::tiles::TileMux::translate_async(
                    tilemng::tilemux(act.tile_id()),
                    act.id(),
                    rbuf_virt,
                    PageFlags::RW,
                )?;
                ktcu::glob_to_phys_remote(act.tile_id(), glob, base::kif::PageFlags::RW).unwrap()
            }
            else {
                rbuf_virt.as_phys(desc)
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
            let rgate = RGateObject::new(cfg::SYSC_RBUF_ORD, cfg::SYSC_RBUF_ORD, false);
            rgate.activate(
                platform::kernel_tile(),
                ktcu::KSYS_EP,
                PhysAddr::new_raw(platform::tile_desc(self.tile_id()), 0xDEADBEEF),
            );
            let _rg_clone = rgate.clone(); // keep one strong reference
            let sgate = SGateObject::new(rgate.downgrade_store(), self.id() as tcu::Label, 1);
            tilemux.config_snd_ep(self.eps_start + tcu::SYSC_SEP_OFF, act, &sgate)?;
        }

        // attach syscall receive endpoint
        let mut rbuf_addr = self.rbuf_phys.get();
        {
            let rgate = RGateObject::new(cfg::SYSC_RBUF_ORD, cfg::SYSC_RBUF_ORD, false);
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
            let rgate = RGateObject::new(cfg::UPCALL_RBUF_ORD, cfg::UPCALL_RBUF_ORD, false);
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
            let rgate = RGateObject::new(cfg::DEF_RBUF_ORD, cfg::DEF_RBUF_ORD, false);
            rgate.activate(self.tile_id(), self.eps_start + tcu::DEF_REP_OFF, rbuf_addr);
            tilemux.config_rcv_ep(self.eps_start + tcu::DEF_REP_OFF, act, None, &rgate)?;
        }

        Ok(())
    }

    pub fn id(&self) -> ActId {
        self.id
    }

    pub fn tile(&self) -> TempRc<TileObject> {
        self.tile.upgrade().unwrap()
    }

    pub fn tile_weak(&self) -> &WeakRc<TileObject> {
        &self.tile
    }

    pub fn tile_id(&self) -> TileId {
        self.tile_id
    }

    pub fn tile_desc(&self) -> TileDesc {
        platform::tile_desc(self.tile_id())
    }

    pub fn kmem(&self) -> Option<TempRc<KMemObject>> {
        self.kmem.upgrade()
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

    pub fn get_kobj<T>(&self, sel: kif::CapSel) -> Result<T, VerboseError>
    where
        T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
    {
        let table = self.obj_caps().borrow();
        table.get_kobj(sel)
    }

    pub fn state(&self) -> State {
        self.state.get()
    }

    pub fn is_dead(&self) -> bool {
        self.state.get() != State::INIT && self.state.get() != State::RUNNING
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

    pub fn add_ep(&self, ep: StrongRc<EPObject>) {
        self.eps.borrow_mut().push(ep.downgrade_store());
    }

    pub fn rem_ep(&self, ep: &TempRc<EPObject>) {
        self.eps
            .borrow_mut()
            .retain(|e| e.upgrade().unwrap().ep() != ep.ep());
    }

    pub fn syscall(&self) -> Ref<'_, OwnedMessage> {
        self.cur_sysc.borrow()
    }

    pub fn set_syscall(&self, msg: OwnedMessage) {
        *self.cur_sysc.borrow_mut() = msg;
    }

    pub fn reply_syscall(&self, reply: &MsgBuf) -> Result<(), Error> {
        // note that we cannot hand out a mutable reference to the OwnedMessage, because that would
        // allow the caller to swap it with something else. Thus, we replicate this method and call
        // reply ourself.
        self.cur_sysc.borrow_mut().reply(reply)
    }

    pub fn start_derive(&self, derive: DeriveSrv) -> Result<(), Error> {
        if self.cur_derive_srv.borrow().is_some() {
            return Err(Error::new(Code::Exists));
        }

        *self.cur_derive_srv.borrow_mut() = Some(derive);
        Ok(())
    }

    pub fn finish_derive(&self) -> Option<DeriveSrv> {
        self.cur_derive_srv.borrow_mut().take()
    }

    fn fetch_exit(&self, sels: &[u64]) -> Result<Option<(CapSel, Code)>, Error> {
        for sel in sels {
            match self
                .obj_caps()
                .borrow()
                .get_kobj::<TempRc<Activity>>(*sel as CapSel)
            {
                Err(e) => return Err(Error::new(e.code())),
                Ok(wv) => {
                    if wv.id() == self.id() {
                        continue;
                    }

                    if let Some(code) = wv.fetch_exit_code() {
                        return Ok(Some((*sel, code)));
                    }
                },
            }
        }

        Ok(None)
    }

    pub fn wait_exit_async(
        act: TempRc<Self>,
        event: u64,
        sels: &[u64],
    ) -> Result<Option<(CapSel, Code)>, Error> {
        let act_id = act.id();
        let act_weak = act.downgrade_asyn();

        let res = loop {
            let act = act_weak
                .upgrade()
                .ok_or_else(|| Error::new(Code::ObjectGone))?;

            // independent of how we notify the activity, check for exits in case the activity we wait for
            // already exited.
            if let Some((sel, code)) = act.fetch_exit(sels)? {
                // if we want to be notified by upcall, do that
                if event != 0 {
                    act.upcall_activity_wait(event, sel, code);
                    // we never report the result via syscall reply, but we need Some for below.
                    break Some((kif::INVALID_SEL, Code::Success));
                }
                else {
                    break Some((sel, code));
                }
            }

            // if we want to be notified by upcall, don't wait, just stop here
            if event != 0 || act.state() != State::RUNNING {
                break None;
            }

            // wait until someone exits
            let event = &EXIT_EVENT as *const _ as thread::Event;
            drop(act);
            thread::wait_for_async(event);
        };

        // ensure that we are removed from the list in any case. we might have started to wait
        // earlier and are now waiting again with a different selector list.
        EXIT_LISTENERS.borrow_mut().retain(|l| l.id != act_id);
        match event {
            // sync wait
            0 => Ok(res),
            // async wait
            _ => {
                // if no one exited yet, remember us
                if !sels.is_empty() && res.is_none() {
                    EXIT_LISTENERS.borrow_mut().push(ExitWait {
                        id: act_id,
                        event,
                        sels: sels.to_vec(),
                    });
                }
                // in any case, the syscall replies "no result"
                Ok(None)
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
            if let Ok(Some((sel, code))) = act.fetch_exit(&l.sels) {
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

    pub fn upcall_derive_srv(&self, event: u64, result: Code) {
        let mut buf = MsgBuf::borrow_def();
        let msg = kif::upcalls::DeriveSrv {
            event,
            error: result,
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

    pub fn start_app_async(act: TempRc<Activity>) -> Result<(), Error> {
        if act.state.get() != State::INIT {
            return Ok(());
        }

        act.state.set(State::RUNNING);

        let id = act.id();
        let tile_id = act.tile_id();
        drop(act);

        ActivityMng::start_activity_async(id, tile_id)
    }

    pub fn stop_app_async(act: TempRc<Activity>, exit_code: Code, revoker: ActId) {
        if act.state.get() == State::DEAD {
            return;
        }

        // safety: we use the pointer as an event while the activity still exists
        let event = TempRc::as_ptr(&act) as usize as thread::Event;

        if act.state.get() == State::STOPPING {
            // if we're in the process of stopping the activity, just wait for that to finish. we
            // want to wait here to ensure that it has been fully stopped and to prevent trouble
            // when doing that with multiple threads concurrently. For example, if one thread
            // already started, has removed a capability and is now waiting for the destruction
            // of the kernel object (e.g., needs a response from TileMux), the second thread will
            // not find the capability anymore and therefore could think that everything is done,
            // but actually it isn't.
            drop(act);
            thread::wait_for_async(event);
        }
        else {
            // mark the activity as "in the process of being stopped"
            let old_state = act.state.get();
            act.state.set(State::STOPPING);

            log!(
                LogFlags::KernActs,
                "Stopping Activity {} [id={}]",
                act.name(),
                act.id()
            );

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
                    ep.deconfigure(InvalidateType::Force).ok();
                }
            }

            // make sure that we don't get further syscalls by this activity
            ktcu::drop_msgs(ktcu::KSYS_EP, act.id() as Label);
            // ensure that we don't access the last syscall anymore (or reply)
            act.cur_sysc.borrow_mut().invalidate();

            let act = if !act.is_root() {
                let act_weak = act.clone().downgrade_asyn();
                Self::revoke_caps_async(act, revoker);
                act_weak.upgrade().unwrap()
            }
            else {
                act
            };

            // don't send stop to accelerators if it exited by itself (which they do via
            // activity_ctrl(STOP))
            let act = if act.tile_desc().is_programmable()
                || (act.state() == State::RUNNING && revoker != act.id())
            {
                let act_weak = act.clone().downgrade_asyn();
                // ignore failures here
                let _ = ActivityMng::stop_activity_async(act);
                act_weak.upgrade().unwrap()
            }
            else {
                act
            };

            // if it's root, there is nobody waiting for it; just remove it
            if act.is_root() {
                tilemng::tilemux(act.tile_id).rem_activity(act.id);
                ActivityMng::remove_activity(act.id());
                thread::remove_thread();
            }

            // change state before the notify
            act.exit_code.set(Some(exit_code)); // TODO exit code when it wasn't running?
            act.state.set(State::DEAD);

            if old_state == State::RUNNING {
                EXIT_LISTENERS.borrow_mut().retain(|l| l.id != act.id());
                Self::send_exit_notify();
            }

            // now that it's completely dead, notify potential other threads that are waiting
            thread::notify(event, None);
        }
    }

    pub fn revoke_caps_async(act: TempRc<Activity>, revoker: ActId) {
        // TODO that's not okay
        let act_rc = unsafe { TempRc::into_strong_unchecked(act) };

        CapTable::revoke_all_async(&act_rc.obj_caps, revoker);
        CapTable::revoke_all_async(&act_rc.map_caps, revoker);
    }

    pub fn revoke_async(&self, crd: CapRngDesc, own: bool, revoker: ActId) {
        // we can't use borrow_mut() here, because revoke might need to use borrow as well.
        if crd.cap_type() == CapType::Object {
            CapTable::revoke_async(self.obj_caps(), crd, own, revoker);
        }
        else {
            CapTable::revoke_async(self.map_caps(), crd, own, revoker);
        }
    }
}

impl_from_kobj!(Activity, Activity);

impl Drop for Activity {
    fn drop(&mut self) {
        self.state.set(State::DEAD);

        // free standard EPs
        tilemng::tilemux(self.tile_id()).free_eps(self.eps_start, STD_EPS_COUNT);
        if let Some(tile) = self.tile.upgrade() {
            tile.free_eps(STD_EPS_COUNT);
            // remove us from tile
            if let Some((aid, sel)) = self.parent {
                tile.rem_activity(aid, sel);
            }
        }

        assert!(self.obj_caps.borrow().is_empty());
        assert!(self.map_caps.borrow().is_empty());

        tilemng::tilemux(self.tile_id).rem_activity(self.id);
        ActivityMng::remove_activity(self.id);

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
