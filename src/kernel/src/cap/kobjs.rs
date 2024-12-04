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

use anyhow::Context;

use base::build_vmsg;
use base::cell::{Cell, Ref, RefCell, RefMut, StaticCell};
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::{self, service, tilemux::QuotaId};
use base::kif::{CapRngDesc, CapSel, CapType};
use base::mem::{size_of, GlobAddr, GlobOff, MsgBuf, MsgBufRef, PhysAddr, VirtAddr};
use base::rc::Rc;
use base::tcu::{ActId, EpId, Label, TileId};
use base::vec::Vec;
use base::{env, tcu};
use base::{format, log};

use thread::{Downgradable, NonWeak, StrongRc, TempRc, Upgradable, WeakRc};

use core::fmt;
use core::ops::Deref;

use crate::com::{SendQueue, Service};
use crate::kerrno;
use crate::ktcu;
use crate::mem;
use crate::platform;
use crate::tiles::{tilemng, Activity, ActivityMng, TileMux};

#[derive(Clone)]
pub enum KObject {
    RGate(StrongRc<RGateObject>),
    SGate(StrongRc<SGateObject>),
    MGate(StrongRc<MGateObject>),
    Map(StrongRc<MapObject>),
    Serv(StrongRc<ServObject>),
    Sess(StrongRc<SessObject>),
    Sem(StrongRc<SemObject>),
    Activity(StrongRc<Activity>),
    KMem(StrongRc<KMemObject>),
    Tile(StrongRc<TileObject>),
    EP(StrongRc<EPObject>),
}

impl KObject {
    pub fn ref_count(&self) -> usize {
        match self {
            KObject::SGate(o) => StrongRc::strong_count(o),
            KObject::RGate(o) => StrongRc::strong_count(o),
            KObject::MGate(o) => StrongRc::strong_count(o),
            KObject::Map(o) => StrongRc::strong_count(o),
            KObject::Serv(o) => StrongRc::strong_count(o),
            KObject::Sess(o) => StrongRc::strong_count(o),
            KObject::Activity(o) => StrongRc::strong_count(o),
            KObject::Sem(o) => StrongRc::strong_count(o),
            KObject::KMem(o) => StrongRc::strong_count(o),
            KObject::Tile(o) => StrongRc::strong_count(o),
            KObject::EP(o) => StrongRc::strong_count(o),
        }
    }
}

pub trait IntoKObject<T> {
    unsafe fn into_kobj(self) -> KObject;
}

#[macro_export]
macro_rules! impl_from_kobj {
    ($ty:ty, $name:ident) => {
        impl TryFrom<&KObject> for TempRc<$ty> {
            type Error = anyhow::Error;

            fn try_from(kobj: &KObject) -> Result<Self, Self::Error> {
                match kobj {
                    KObject::$name(s) => Ok(thread::TempRc::new(s.clone())),
                    _ => Err($crate::kerrno(base::errors::Code::InvArgs)
                        .context(concat!("Expected ", stringify!($name)))),
                }
            }
        }

        impl IntoKObject<$ty> for StrongRc<$ty> {
            unsafe fn into_kobj(self) -> KObject {
                KObject::$name(self)
            }
        }
    };
}

const fn kobj_size<T>() -> usize {
    let size = size_of::<T>();
    if size <= 64 {
        64 + crate::slab::HEADER_SIZE
    }
    else if size <= 128 {
        128 + crate::slab::HEADER_SIZE
    }
    else {
        // since we are using musl's heap, it's hard to say what the overhead per allocation is.
        // that depends on whether we needed a new "group" or not, for example. as an estimate use
        // 64 bytes.
        size + 64
    }
}

static KOBJ_SIZES: [usize; 11] = [
    kobj_size::<SGateObject>(),
    kobj_size::<RGateObject>(),
    kobj_size::<MGateObject>(),
    kobj_size::<MapObject>(),
    kobj_size::<ServObject>(),
    kobj_size::<SessObject>(),
    kobj_size::<Activity>(),
    kobj_size::<SemObject>(),
    kobj_size::<KMemObject>(),
    // assume pessimistically that each TileObject has its own EPQuota
    kobj_size::<TileObject>() + kobj_size::<TileQuota>(),
    kobj_size::<EPObject>(),
];

impl KObject {
    pub fn size(&self) -> usize {
        // get the index in the enum
        let idx: usize = unsafe { *(self as *const _ as *const usize) };
        KOBJ_SIZES[idx]
    }
}

fn fmt_kobj<T: fmt::Debug>(f: &mut fmt::Formatter<'_>, o: &StrongRc<T>) -> fmt::Result {
    write!(
        f,
        "{:?}; @{:#x}; refs={}",
        o,
        StrongRc::as_ptr(o) as usize,
        StrongRc::strong_count(o),
    )
}

impl fmt::Debug for KObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KObject::SGate(o) => fmt_kobj(f, o),
            KObject::RGate(o) => fmt_kobj(f, o),
            KObject::MGate(o) => fmt_kobj(f, o),
            KObject::Map(o) => fmt_kobj(f, o),
            KObject::Serv(o) => fmt_kobj(f, o),
            KObject::Sess(o) => fmt_kobj(f, o),
            KObject::Activity(o) => fmt_kobj(f, o),
            KObject::Sem(o) => fmt_kobj(f, o),
            KObject::KMem(o) => fmt_kobj(f, o),
            KObject::Tile(o) => fmt_kobj(f, o),
            KObject::EP(o) => fmt_kobj(f, o),
        }
    }
}

pub struct GateEP {
    ep: WeakRc<EPObject>,
}

impl GateEP {
    fn new() -> Self {
        Self {
            ep: WeakRc::default(),
        }
    }

    pub fn get_ep(&self) -> Option<TempRc<EPObject>> {
        self.ep.upgrade()
    }

    pub fn set_ep(&mut self, o: TempRc<EPObject>) {
        self.ep = o.downgrade_store();
    }

    pub fn remove_ep(&mut self) {
        self.ep = WeakRc::default()
    }
}

pub enum GateObject {
    Recv(WeakRc<RGateObject>),
    Send(WeakRc<SGateObject>),
    Mem(WeakRc<MGateObject>),
}

pub struct BaseGate {
    gep: RefCell<GateEP>,
}

impl BaseGate {
    pub fn set_ep(&self, ep: &TempRc<EPObject>, gobj: GateObject) {
        self.gep.borrow_mut().set_ep(ep.clone());
        ep.set_gate(gobj);
    }

    pub fn gate_ep(&self) -> Ref<'_, GateEP> {
        self.gep.borrow()
    }

    pub fn gate_ep_mut(&self) -> RefMut<'_, GateEP> {
        self.gep.borrow_mut()
    }
}

impl Default for BaseGate {
    fn default() -> Self {
        Self {
            gep: RefCell::from(GateEP::new()),
        }
    }
}

pub struct RGateObject {
    base: BaseGate,
    loc: Cell<Option<(TileId, EpId)>>,
    addr: Cell<PhysAddr>,
    order: u32,
    msg_order: u32,
    serial: bool,
}

impl RGateObject {
    pub fn new(order: u32, msg_order: u32, serial: bool) -> StrongRc<Self> {
        StrongRc::new(Self {
            base: BaseGate::default(),
            loc: Cell::from(None),
            addr: Cell::from(PhysAddr::default()),
            order,
            msg_order,
            serial,
        })
    }

    pub fn location(&self) -> Option<(TileId, EpId)> {
        self.loc.get()
    }

    pub fn addr(&self) -> PhysAddr {
        self.addr.get()
    }

    pub fn order(&self) -> u32 {
        self.order
    }

    pub fn size(&self) -> usize {
        1 << self.order
    }

    pub fn msg_order(&self) -> u32 {
        self.msg_order
    }

    pub fn msg_size(&self) -> usize {
        1 << self.msg_order
    }

    pub fn activated(&self) -> bool {
        self.addr.get() != PhysAddr::default()
    }

    pub fn activate(&self, tile: TileId, ep: EpId, addr: PhysAddr) {
        self.loc.replace(Some((tile, ep)));
        self.addr.replace(addr);
        if self.serial {
            crate::platform::init_serial(Some((tile, ep)));
        }
    }

    pub fn deactivate(&self) {
        self.addr.set(PhysAddr::default());
        self.loc.set(None);
        if self.serial {
            crate::platform::init_serial(None);
        }
    }

    pub fn get_event(&self) -> thread::Event {
        self as *const Self as thread::Event
    }

    pub fn print_loc(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.loc.get() {
            Some((tile, ep)) => write!(f, "{}:EP{}", tile, ep),
            None => write!(f, "?"),
        }
    }
}

impl_from_kobj!(RGateObject, RGate);

impl Deref for RGateObject {
    type Target = BaseGate;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl fmt::Debug for RGateObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RGate[loc=")?;
        self.print_loc(f)?;
        write!(
            f,
            ", addr={}, sz={:#x}, msz={:#x}]",
            self.addr.get(),
            self.size(),
            self.msg_size()
        )
    }
}

pub struct SGateObject {
    base: BaseGate,
    rgate: WeakRc<RGateObject>,
    label: Label,
    credits: u32,
}

impl SGateObject {
    pub fn new(rgate: WeakRc<RGateObject>, label: Label, credits: u32) -> StrongRc<Self> {
        StrongRc::new(Self {
            base: BaseGate::default(),
            rgate,
            label,
            credits,
        })
    }

    pub fn rgate(&self) -> Option<TempRc<RGateObject>> {
        self.rgate.upgrade()
    }

    pub fn label(&self) -> Label {
        self.label
    }

    pub fn credits(&self) -> u32 {
        self.credits
    }

    pub fn invalidate_reply_eps(&self) {
        // is the send gate activated?
        if let Some(sep) = self.gate_ep().get_ep() {
            // is the associated receive gate activated?
            if let Some((recv_tile, recv_ep)) = self.rgate().and_then(|rg| rg.location()) {
                let tilemux = tilemng::tilemux(sep.tile_id());
                tilemux
                    .invalidate_reply_eps(recv_tile, recv_ep, sep.ep())
                    .unwrap();
            }
        }
    }
}

impl_from_kobj!(SGateObject, SGate);

impl Deref for SGateObject {
    type Target = BaseGate;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl fmt::Debug for SGateObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SGate[rgate=")?;
        match self.rgate() {
            Some(rg) => rg.print_loc(f)?,
            None => write!(f, "?")?,
        }
        write!(f, ", lbl={:#x}, crd={}]", self.label, self.credits)
    }
}

pub struct MGateObject {
    base: BaseGate,
    mem: mem::Allocation,
    perms: kif::Perm,
    exclusive: Cell<Option<TileId>>,
    derived: bool,
}

impl MGateObject {
    pub fn new(mem: mem::Allocation, perms: kif::Perm, derived: bool) -> StrongRc<Self> {
        StrongRc::new(Self {
            base: BaseGate::default(),
            mem,
            perms,
            exclusive: Cell::from(None),
            derived,
        })
    }

    pub fn tile_id(&self) -> TileId {
        self.mem.global().tile()
    }

    pub fn offset(&self) -> GlobOff {
        self.mem.global().offset()
    }

    pub fn addr(&self) -> GlobAddr {
        self.mem.global()
    }

    pub fn size(&self) -> GlobOff {
        self.mem.size()
    }

    pub fn perms(&self) -> kif::Perm {
        self.perms
    }

    pub fn exclusive_tile(&self) -> Option<TileId> {
        self.exclusive.get()
    }

    pub fn derive(
        &self,
        offset: GlobOff,
        size: GlobOff,
        perms: kif::Perm,
    ) -> anyhow::Result<StrongRc<Self>> {
        if offset.checked_add(size).is_none() || offset + size > self.size() || size == 0 {
            return Err(kerrno(Code::InvArgs).context("Size or offset invalid"));
        }

        let addr = self.addr().raw() + offset;
        let mem = mem::Allocation::new(GlobAddr::new(addr), size);

        Ok(StrongRc::new(Self {
            base: BaseGate::default(),
            mem,
            perms: self.perms & perms,
            exclusive: self.exclusive.clone(),
            derived: true,
        }))
    }

    pub fn make_exclusive(&self, user_tile: &TempRc<TileObject>) -> anyhow::Result<()> {
        if let Some(extile) = self.exclusive.get() {
            if extile == user_tile.tile() {
                return Ok(());
            }
            return Err(kerrno(Code::Exists).context("MGate is exclusive"));
        }

        self.exclusive.set(Some(user_tile.tile()));
        Ok(())
    }

    pub fn inval_exclusive(&self) {
        self.exclusive.take();
    }
}

impl_from_kobj!(MGateObject, MGate);

impl Drop for MGateObject {
    fn drop(&mut self) {
        // if it's not derived, it's always memory from mem-tiles
        if !self.derived {
            mem::borrow_mut().free(&self.mem);
        }
    }
}

impl Deref for MGateObject {
    type Target = BaseGate;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl fmt::Debug for MGateObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MGate[tile={}, addr={}, size={:#x}, perm={:?}, der={}]",
            self.tile_id(),
            self.addr(),
            self.size(),
            self.perms,
            self.derived
        )
    }
}

pub struct ServObject {
    // note: this Rc should not leak outside of this object to prevent that anyone accidentally
    // keeps a reference across an async call
    serv: Rc<Service>,
    owner: bool,
    creator: usize,
}

impl ServObject {
    pub fn new(serv: Rc<Service>, owner: bool, creator: usize) -> StrongRc<Self> {
        StrongRc::new(Self {
            serv,
            owner,
            creator,
        })
    }

    pub fn name(&self) -> &str {
        self.serv.name()
    }

    pub fn server_act(&self) -> TempRc<Activity> {
        self.serv.activity()
    }

    pub fn creator(&self) -> usize {
        self.creator
    }

    pub fn set_derive_act(&self, act: TempRc<Activity>) -> anyhow::Result<()> {
        self.serv.set_derive_act(act)
    }

    pub fn fetch_derive_act(&self) -> anyhow::Result<TempRc<Activity>> {
        self.serv.fetch_derive_act()
    }

    pub fn derive(&self, creator: usize) -> StrongRc<Self> {
        Self::new(self.serv.clone(), false, creator)
    }

    pub fn send(&self, lbl: Label, msg: MsgBufRef) -> anyhow::Result<thread::Event> {
        self.serv.send(lbl, &msg)
    }

    pub fn send_receive_async(
        srv: TempRc<Self>,
        lbl: Label,
        msg: MsgBufRef,
    ) -> anyhow::Result<&'static tcu::Message> {
        let event = Self::send(&srv, lbl, msg)?;
        drop(srv);
        SendQueue::receive_async(event)
    }

    pub fn abort(&self) {
        if self.owner {
            self.serv.abort();
        }
    }
}

impl_from_kobj!(ServObject, Serv);

impl fmt::Debug for ServObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Serv[srv={:?}, owner={}, creator={}]",
            self.serv, self.owner, self.creator
        )
    }
}

pub struct SessObject {
    srv: WeakRc<ServObject>,
    creator: usize,
    ident: u64,
    auto_close: bool,
}

impl SessObject {
    pub fn new(
        srv: WeakRc<ServObject>,
        creator: usize,
        ident: u64,
        auto_close: bool,
    ) -> StrongRc<Self> {
        StrongRc::new(Self {
            srv,
            creator,
            ident,
            auto_close,
        })
    }

    pub fn service(&self) -> Option<TempRc<ServObject>> {
        self.srv.upgrade()
    }

    pub fn creator(&self) -> usize {
        self.creator
    }

    pub fn ident(&self) -> u64 {
        self.ident
    }

    pub fn close(&self, revoker: ActId) {
        if self.auto_close {
            if let Some(serv) = self.service() {
                // don't send the close, if the server is the revoker
                if serv.server_act().id() == revoker {
                    return;
                }

                log!(
                    LogFlags::KernServ,
                    "Sending close(sess={:#x}) to service {} with creator {}",
                    self.ident(),
                    serv.name(),
                    self.creator,
                );

                let mut smsg = MsgBuf::borrow_def();
                build_vmsg!(smsg, service::Request::Close { sid: self.ident });

                let creator = self.creator as Label;

                // this should never fail, because the close request fails only if the creator does not
                // own the session. but we know here that the creator owns this session.
                if let Err(e) = serv.send(creator, smsg) {
                    log!(LogFlags::Error, "Session-close request failed: {}", e);
                }
            }
        }
    }
}

impl_from_kobj!(SessObject, Sess);

impl fmt::Debug for SessObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Sess[service={}, creator={}, ident={:#x}]",
            match self.service().as_ref() {
                Some(s) => s.name(),
                None => "?",
            },
            self.creator,
            self.ident,
        )
    }
}

pub struct SemObject {
    counter: Cell<u32>,
    waiters: Cell<u32>,
}

impl SemObject {
    pub fn new(counter: u32) -> StrongRc<Self> {
        StrongRc::new(Self {
            counter: Cell::from(counter),
            waiters: Cell::from(0),
        })
    }

    pub fn down_async(s: TempRc<Self>) -> anyhow::Result<()> {
        let sem_weak = s.downgrade_asyn();
        loop {
            let sem = sem_weak
                .upgrade()
                .ok_or_else(|| kerrno(Code::ObjectGone).context("Semaphore was destroyed"))?;
            if sem.counter.get() != 0 {
                sem.counter.set(sem.counter.get() - 1);
                break;
            }

            sem.waiters.set(sem.waiters.get() + 1);
            let event = sem.get_event();
            let tmp_weak = sem.downgrade_asyn();

            thread::wait_for_async(event);

            let sem = tmp_weak
                .upgrade()
                .ok_or_else(|| kerrno(Code::ObjectGone).context("Semaphore was destroyed"))?;
            sem.waiters.set(sem.waiters.get() - 1);
        }
        Ok(())
    }

    pub fn up(&self) {
        if self.waiters.get() > 0 {
            thread::notify(self.get_event(), None);
        }
        self.counter.set(self.counter.get() + 1);
    }

    pub fn revoke(&self) {
        if self.waiters.get() > 0 {
            thread::notify(self.get_event(), None);
        }
    }

    fn get_event(&self) -> thread::Event {
        self as *const Self as thread::Event
    }
}

impl_from_kobj!(SemObject, Sem);

impl fmt::Debug for SemObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Sem[counter={}, waiters={}]",
            self.counter.get(),
            self.waiters.get()
        )
    }
}

pub struct TileQuota {
    id: QuotaId,
    total: Cell<usize>,
    left: Cell<usize>,
}

impl TileQuota {
    pub fn new(num: usize) -> Rc<Self> {
        static NEXT_ID: StaticCell<QuotaId> = StaticCell::new(0);
        let id = NEXT_ID.get();
        NEXT_ID.set(id + 1);

        Rc::new(Self {
            id,
            total: Cell::from(num),
            left: Cell::from(num),
        })
    }

    pub fn id(&self) -> QuotaId {
        self.id
    }

    pub fn total(&self) -> usize {
        self.total.get()
    }

    pub fn left(&self) -> usize {
        self.left.get()
    }

    fn alloc(&self, num: usize, tile: TileId, name: &str) {
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: allocating {} {} ({} left)",
            tile,
            self as *const _ as usize,
            num,
            name,
            self.left()
        );
        assert!(self.left() >= num);
        self.left.set(self.left() - num);
    }

    fn free(&self, num: usize, tile: TileId, name: &str) {
        assert!(self.left() + num <= self.total());
        self.left.set(self.left() + num);
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: freed {} {} ({} left)",
            tile,
            self as *const _ as usize,
            num,
            name,
            self.left()
        );
    }
}

pub struct TileObject {
    tile: TileId,
    acts: RefCell<Vec<(ActId, CapSel)>>,
    ep_quota: Rc<TileQuota>,
    exregs_quota: Rc<TileQuota>,
    time_quota: QuotaId,
    pt_quota: QuotaId,
    derived: bool,
}

impl TileObject {
    pub fn new(
        tile: TileId,
        ep_quota: Rc<TileQuota>,
        exregs_quota: Rc<TileQuota>,
        time_quota: QuotaId,
        pt_quota: QuotaId,
        derived: bool,
    ) -> StrongRc<Self> {
        let res = StrongRc::new(Self {
            tile,
            acts: RefCell::from(Vec::new()),
            ep_quota: ep_quota.clone(),
            exregs_quota: exregs_quota.clone(),
            time_quota,
            pt_quota,
            derived,
        });
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: {} new TileObject with EPs={}, exregs={}, time={}, pts={}",
            tile,
            &*res as *const _ as usize,
            if derived { "derived" } else { "created" },
            ep_quota.total(),
            exregs_quota.total(),
            time_quota,
            pt_quota,
        );
        res
    }

    pub fn derive_async(
        tile: TempRc<Self>,
        eps: Option<usize>,
        exregs: Option<usize>,
        time: Option<u64>,
        pts: Option<usize>,
    ) -> anyhow::Result<StrongRc<Self>> {
        // only allocate it from the tile here, but don't keep an Rc to the EPQuota
        if let Some(num) = eps {
            if !tile.has_quota(num) {
                return Err(kerrno(Code::NoSpace).context("Insufficient EPs for tile derive"));
            }
            tile.alloc_eps(num);
        }

        let tile_id = tile.tile();
        let (time_id, pt_id, tile) = if platform::tile_desc(tile_id).supports_tilemux()
            && (time.is_some() || pts.is_some())
        {
            let tilemux = tilemng::tilemux(tile_id);
            let time_quota_id = tile.time_quota_id();
            let pt_quota_id = tile.pt_quota_id();
            let tile_weak = tile.downgrade_asyn();

            let res = TileMux::derive_quota_async(tilemux, time_quota_id, pt_quota_id, time, pts);

            // note that we don't need to give the EP quota back to the tile as the tile was
            // destroyed in this case, meaning that we already gave the quota back in
            // TileObject::revoke_async.
            let tile = tile_weak
                .upgrade()
                .ok_or_else(|| kerrno(Code::ObjectGone).context("Tile was destroyed"))?;

            match res {
                Err(e) => {
                    if let Some(num) = eps {
                        tile.free_eps(num);
                    }
                    return Err(e.context("TileMux declined tile derive"));
                },
                Ok(v) => (v.0, v.1, tile),
            }
        }
        else {
            (tile.time_quota_id(), tile.pt_quota_id(), tile)
        };

        // now that the async call is done, create the EPQuota
        let ep_quota = if let Some(num) = eps {
            TileQuota::new(num)
        }
        else {
            tile.ep_quota.clone()
        };
        let exregs_quota = if let Some(num) = exregs {
            TileQuota::new(num)
        }
        else {
            tile.exregs_quota.clone()
        };
        Ok(Self::new(
            tile_id,
            ep_quota,
            exregs_quota,
            time_id,
            pt_id,
            true,
        ))
    }

    pub fn tile(&self) -> TileId {
        self.tile
    }

    pub fn derived(&self) -> bool {
        self.derived
    }

    pub fn activities(&self) -> usize {
        self.acts.borrow().len()
    }

    pub fn ep_quota(&self) -> &TileQuota {
        self.ep_quota.as_ref()
    }

    pub fn exregs_quota(&self) -> &TileQuota {
        self.exregs_quota.as_ref()
    }

    pub fn time_quota_id(&self) -> QuotaId {
        self.time_quota
    }

    pub fn pt_quota_id(&self) -> QuotaId {
        self.pt_quota
    }

    pub fn has_quota(&self, eps: usize) -> bool {
        self.ep_quota.left() >= eps
    }

    pub fn add_activity(&self, act: ActId, sel: CapSel) {
        self.acts.borrow_mut().push((act, sel));
    }

    pub fn rem_activity(&self, act: ActId, sel: CapSel) {
        // note that we might not find it in the list in case we triggered the activity revoke from
        // below as this already removes itself in its drop implementation.
        self.acts
            .borrow_mut()
            .retain(|(a, s)| !(*a == act && *s == sel));
    }

    pub fn memory(&self) -> mem::Allocation {
        let desc = platform::tile_desc(self.tile());
        // on the hw platform we cannot write into the local memory until the core is running.
        // however, we cannot turn on the core until we have properly initialized the memory. thus,
        // we need to write it to the DRAM location that emulates local SPM on the hw platform.
        if env::boot().platform == env::Platform::Hw {
            match ktcu::unpack_mem_ep_remote(self.tile(), 0) {
                // if we have a valid memory EP in EP0, we have emulated SPM
                Ok((mem_tile, mem_off, mem_size, _perm)) => {
                    mem::Allocation::new(GlobAddr::new_with(mem_tile, mem_off), mem_size)
                },
                // otherwise we have real SPM
                Err(e) if e.downcast_ref::<Error>().unwrap().code() == Code::NoMEP => {
                    mem::Allocation::new(
                        GlobAddr::new_with(self.tile(), desc.mem_offset() as GlobOff),
                        desc.mem_size() as GlobOff,
                    )
                },
                Err(e) => panic!("Unable to read PMPEP0: {}", e),
            }
        }
        else {
            mem::Allocation::new(
                GlobAddr::new_with(self.tile(), desc.mem_offset() as GlobOff),
                desc.mem_size() as GlobOff,
            )
        }
    }

    pub fn alloc_eps(&self, num: usize) {
        self.ep_quota.alloc(num, self.tile, "EPs");
    }

    pub fn free_eps(&self, num: usize) {
        self.ep_quota.free(num, self.tile, "EPs");
    }

    pub fn alloc_exreg(&self, num: usize) {
        self.exregs_quota.alloc(num, self.tile, "ExRegs");
    }

    pub fn free_exregs(&self, num: usize) {
        self.exregs_quota.free(num, self.tile, "ExRegs");
    }

    pub fn reset(&self, total_eps: usize) {
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: reset with EPs={}",
            self.tile,
            self as *const _ as usize,
            total_eps,
        );
        self.ep_quota.total.set(total_eps);
        self.ep_quota.left.set(total_eps);
    }

    pub fn revoke_async(&self, parent: TempRc<TileObject>, revoker: ActId) {
        let parent_weak = parent.downgrade_asyn();
        // first revoke all activities that are using this tile
        loop {
            let res = self.acts.borrow_mut().pop();
            match res {
                Some((aid, sel)) => {
                    if let Some(act) = ActivityMng::activity(aid) {
                        // TODO that's not okay
                        let act_rc = unsafe { TempRc::into_strong_unchecked(act) };
                        // note that we deliberately revoke the activity from its parent to make it
                        // behave as if the activity (the original, owned by the parent) was
                        // derived from the tile object.
                        act_rc.revoke_async(
                            CapRngDesc::new_single(CapType::Object, sel),
                            true,
                            revoker,
                        );
                    }
                },
                None => break,
            }
        }

        let Some(parent) = parent_weak.upgrade()
        else {
            return;
        };

        // same for time and pts: free the ones that are different
        let time = if self.time_quota != parent.time_quota {
            Some(self.time_quota)
        }
        else {
            None
        };
        let pts = if self.pt_quota != parent.pt_quota {
            Some(self.pt_quota)
        }
        else {
            None
        };

        // note that we first let TileMux remove the quotas and afterwards give the EPQuota back to
        // our parent to avoid that someone can already spent the EPQuota for something new.
        let parent = if time.is_some() || pts.is_some() {
            let tile_id = self.tile();
            let parent_weak = parent.downgrade_asyn();

            TileMux::remove_quotas_async(tilemng::tilemux(tile_id), time, pts).ok();

            // if that fails, someone else removed the object in the meantime and we can stop here
            // (for example, child cap is revoked first, gets stuck in the async call below, and
            // the parent cap is revoked in the meantime)
            match parent_weak.upgrade() {
                Some(parent) => parent,
                None => return,
            }
        }
        else {
            parent
        };

        // we free the EP quota if it's different from our parent's quota (only our own childs can
        // have the same EP quota, but they are already gone).
        if !Rc::ptr_eq(&self.ep_quota, &parent.ep_quota) {
            // grant the EPs back to our parent
            parent.free_eps(self.ep_quota.left());
            assert!(self.ep_quota.left() == self.ep_quota.total());
        }

        if !Rc::ptr_eq(&self.exregs_quota, &parent.exregs_quota) {
            parent.free_exregs(self.exregs_quota.left());
            assert!(self.exregs_quota.left() == self.exregs_quota.total());
        }
    }
}

impl_from_kobj!(TileObject, Tile);

impl fmt::Debug for TileObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tile[id={}, eps={}, exregs={}, derived={}, acts=",
            self.tile,
            self.ep_quota.left(),
            self.exregs_quota.left(),
            self.derived,
        )?;
        for (aid, sel) in self.acts.borrow().iter() {
            write!(f, "({},{}),", aid, sel)?;
        }
        write!(f, "]")
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum EPCategory {
    PMP,
    Std,
    Custom,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum InvalidateType {
    None,
    Default,
    Force,
}

pub struct EPObject {
    cat: EPCategory,
    gate: RefCell<Option<GateObject>>,
    act: WeakRc<Activity>,
    ep: EpId,
    replies: usize,
    // keep a separate copy of the TileId, because this does never change and if we have a valid
    // reference to an EPObject, the TileObject is always valid as well.
    tile_id: TileId,
    tile: WeakRc<TileObject>,
}

impl EPObject {
    pub fn new(
        cat: EPCategory,
        act: WeakRc<Activity>,
        ep: EpId,
        replies: usize,
        tile: WeakRc<TileObject>,
    ) -> StrongRc<Self> {
        let maybe_act = act.upgrade();
        let ep = StrongRc::new(Self {
            cat,
            gate: RefCell::from(None),
            act,
            ep,
            replies,
            tile_id: tile.upgrade().unwrap().tile(),
            tile,
        });
        if let Some(v) = maybe_act {
            v.add_ep(ep.clone());
        }
        ep
    }

    pub fn tile_id(&self) -> TileId {
        self.tile_id
    }

    pub fn activity(&self) -> Option<TempRc<Activity>> {
        self.act.upgrade()
    }

    pub fn ep(&self) -> EpId {
        self.ep
    }

    pub fn replies(&self) -> usize {
        self.replies
    }

    pub fn is_rgate(&self) -> bool {
        matches!(self.gate.borrow().as_ref(), Some(GateObject::Recv(_)))
    }

    pub fn set_gate(&self, g: GateObject) {
        self.gate.replace(Some(g));
    }

    pub fn revoke(ep: TempRc<Self>) {
        if let Some(v) = ep.act.upgrade() {
            v.rem_ep(&ep);
        }
    }

    pub fn is_configured(&self) -> bool {
        self.gate.borrow().is_some()
    }

    pub fn deconfigure(&self, invalidate: InvalidateType) -> anyhow::Result<bool> {
        let mut invalidated = false;
        if let Some(ref gate) = self.gate.borrow_mut().take() {
            let tile_id = self.tile_id();

            // invalidate receive and send EPs
            match (invalidate, gate) {
                // if no invalidation is requested and it's a memory EP, there is nothing to check
                (InvalidateType::None, GateObject::Mem(_)) => {},

                // otherwise we always invalidate and potentially even force-invalidate
                _ => {
                    tilemng::tilemux(tile_id)
                        .invalidate_ep(
                            self.activity().unwrap().id(),
                            self.ep,
                            invalidate == InvalidateType::Force,
                            true,
                        )
                        .with_context(|| {
                            format!("Invalidating EP {}:{} failed", tile_id, self.ep)
                        })?;
                    invalidated = true;
                },
            }

            match gate {
                GateObject::Send(s) => {
                    if let Some(s) = s.upgrade() {
                        // invalidate reply EPs
                        s.invalidate_reply_eps();
                        // tell the gate that it's no longer valid
                        s.gep.borrow_mut().remove_ep();
                    }
                },
                GateObject::Recv(r) => {
                    if let Some(r) = r.upgrade() {
                        // deactivate receive gate
                        r.deactivate();
                        r.gep.borrow_mut().remove_ep();
                    }
                },
                GateObject::Mem(m) => {
                    if let Some(m) = m.upgrade() {
                        m.gep.borrow_mut().remove_ep();
                    }
                },
            }
        }
        Ok(invalidated)
    }
}

impl_from_kobj!(EPObject, EP);

impl Drop for EPObject {
    fn drop(&mut self) {
        if self.cat == EPCategory::Custom {
            if let Some(tile) = self.tile.upgrade() {
                tilemng::tilemux(tile.tile).free_eps(self.ep, 1 + self.replies);

                tile.free_eps(1 + self.replies);
            }
        }
    }
}

impl fmt::Debug for EPObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EP[act={}, ep={}, replies={}, tile={:?}]",
            self.activity().unwrap().id(),
            self.ep,
            self.replies,
            *self.tile.upgrade().unwrap()
        )
    }
}

pub struct KMemObject {
    id: QuotaId,
    quota: usize,
    left: Cell<usize>,
}

impl KMemObject {
    pub fn new(quota: usize) -> StrongRc<Self> {
        static NEXT_ID: StaticCell<QuotaId> = StaticCell::new(0);
        let id = NEXT_ID.get();
        NEXT_ID.set(id + 1);

        let kmem = StrongRc::new(Self {
            id,
            quota,
            left: Cell::from(quota),
        });
        log!(LogFlags::KernKMem, "{:?} created", *kmem);
        kmem
    }

    pub fn id(&self) -> QuotaId {
        self.id
    }

    pub fn quota(&self) -> usize {
        self.quota
    }

    pub fn left(&self) -> usize {
        self.left.get()
    }

    pub fn has_quota(&self, size: usize) -> bool {
        self.left.get() >= size
    }

    pub fn alloc(&self, act: &Activity, sel: kif::CapSel, size: usize) -> bool {
        log!(
            LogFlags::KernKMem,
            "{:?} Activity{}:{} allocates {}b (sel={})",
            self,
            act.id(),
            act.name(),
            size,
            sel,
        );

        if self.has_quota(size) {
            self.left.set(self.left() - size);
            true
        }
        else {
            false
        }
    }

    pub fn free(&self, act: &Activity, sel: kif::CapSel, size: usize) {
        assert!(self.left() + size <= self.quota);
        self.left.set(self.left() + size);

        log!(
            LogFlags::KernKMem,
            "{:?} Activity{}:{} freed {}b (sel={})",
            self,
            act.id(),
            act.name(),
            size,
            sel
        );
    }

    pub fn revoke(&self, act: &Activity, sel: kif::CapSel, parent: TempRc<KMemObject>) {
        // grant the kernel memory back to our parent
        parent.free(act, sel, self.left());
        assert!(self.left() == self.quota);
    }
}

impl_from_kobj!(KMemObject, KMem);

impl fmt::Debug for KMemObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "KMem[id={}, quota={}, left={}]",
            self.id,
            self.quota,
            self.left()
        )
    }
}

impl Drop for KMemObject {
    fn drop(&mut self) {
        log!(LogFlags::KernKMem, "{:?} dropped", self);
        // don't complain for the first (root's kmem), because here we can't give all quota back as
        // we might destroy the last reference to the kmem object before.
        if self.id != 0 {
            assert!(self.left() == self.quota);
        }
    }
}

pub struct MapObject {
    glob: Cell<GlobAddr>,
    flags: Cell<kif::PageFlags>,
    mapped: Cell<bool>,
}

impl MapObject {
    pub fn new(glob: GlobAddr, flags: kif::PageFlags) -> StrongRc<Self> {
        StrongRc::new(Self {
            glob: Cell::from(glob),
            flags: Cell::from(flags),
            mapped: Cell::from(false),
        })
    }

    pub fn mapped(&self) -> bool {
        self.mapped.get()
    }

    pub fn global(&self) -> GlobAddr {
        self.glob.get()
    }

    pub fn flags(&self) -> kif::PageFlags {
        self.flags.get()
    }

    pub fn map_async(
        map: TempRc<Self>,
        act_id: ActId,
        act_tile: TileId,
        virt: VirtAddr,
        glob: GlobAddr,
        pages: usize,
        flags: kif::PageFlags,
    ) -> anyhow::Result<()> {
        let map_weak = map.downgrade_asyn();
        TileMux::map_async(tilemng::tilemux(act_tile), act_id, virt, glob, pages, flags).map(|_| {
            if let Some(map) = map_weak.upgrade() {
                // TODO note that this is racy (in theory) with other map and unmap (revoke) calls.
                // this does not happen currently, as the pager is the single responsible entity
                // for a given address space and does never hand out mapping capabilities to
                // others. Therefore, all these operations are done by the pager and as there is
                // only one syscall at a time, these races are not possible.
                map.glob.replace(glob);
                map.flags.replace(flags);
                map.mapped.set(true);
            }
        })
    }

    pub fn unmap_async(act_id: ActId, act_tile: TileId, virt: VirtAddr, pages: usize) {
        TileMux::unmap_async(tilemng::tilemux(act_tile), act_id, virt, pages).ok();
    }
}

impl_from_kobj!(MapObject, Map);

impl fmt::Debug for MapObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Map[glob={}, flags={:#x}]", self.global(), self.flags())
    }
}
