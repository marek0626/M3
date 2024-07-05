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

use base::cell::{Cell, Ref, RefCell, RefMut, StaticCell, StaticRefCell};
use base::env;
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::{self, service, tilemux::QuotaId};
use base::log;
use base::mem::{size_of, GlobAddr, GlobOff, MsgBuf, PhysAddr, VirtAddr};
use base::rc::{Rc, SRc, Weak};
use base::tcu::{ActId, EpId, Label, TileId};
use base::{backtrace, build_vmsg};
use thread::Event;

use core::fmt;
use core::ops::Deref;

use crate::com::Service;
use crate::ktcu;
use crate::mem;
use crate::platform;
use crate::tiles::{tilemng, Activity, State, TileMux};

const MAX_TRACE_LEN: usize = 8;
const MAX_TRACES: usize = 8;

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

static OWNED_REFS: StaticCell<u64> = StaticCell::new(0);
static REF_TRACES: StaticRefCell<Traces> = StaticRefCell::new(Traces::new());
const DEBUG_TRACES: bool = true;

fn inc_owned_refs() {
    OWNED_REFS.set(OWNED_REFS.get() + 1);
    if DEBUG_TRACES {
        REF_TRACES.borrow_mut().push();
    }
}

fn dec_owned_refs() {
    assert!(OWNED_REFS.get() > 0);
    OWNED_REFS.set(OWNED_REFS.get() - 1);
    if DEBUG_TRACES {
        REF_TRACES.borrow_mut().pop();
    }
}

pub fn wait_for_async(event: Event) {
    if OWNED_REFS.get() != 0 {
        log!(
            LogFlags::Error,
            "Async call with {} owned reference(s)",
            OWNED_REFS.get()
        );
        if DEBUG_TRACES {
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

    thread::wait_for(event);
}

pub struct KObjectWeakRef<T> {
    obj: Weak<T>,
}

impl<T> KObjectWeakRef<T> {
    pub fn can_upgrade(&self) -> bool {
        self.obj.strong_count() > 0
    }

    pub fn upgrade(&self) -> Option<KObjectOwnedRef<T>> {
        self.obj.upgrade().map(|o| KObjectOwnedRef::new(o))
    }
}

pub struct KObjectOwnedRef<T> {
    obj: Rc<T>,
}

impl<T> KObjectOwnedRef<T> {
    pub fn new(obj: Rc<T>) -> Self {
        inc_owned_refs();
        Self { obj }
    }

    // TODO maybe this should take "self" or so?
    pub fn inner(&self) -> &Rc<T> {
        &self.obj
    }

    pub fn downgrade(self) -> KObjectWeakRef<T> {
        // count will be decreased in drop of self
        KObjectWeakRef {
            obj: Rc::downgrade(&self.obj),
        }
    }
}

impl<T> Clone for KObjectOwnedRef<T> {
    fn clone(&self) -> Self {
        Self::new(self.obj.clone())
    }
}

impl<T> Drop for KObjectOwnedRef<T> {
    fn drop(&mut self) {
        dec_owned_refs();
    }
}

impl<T> Deref for KObjectOwnedRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.obj.deref()
    }
}

pub struct KObjectGenRef {
    obj: KObject,
}

impl KObjectGenRef {
    pub fn new(obj: KObject) -> Self {
        inc_owned_refs();
        Self { obj }
    }

    pub fn get(&self) -> &KObject {
        &self.obj
    }
}

impl Drop for KObjectGenRef {
    fn drop(&mut self) {
        dec_owned_refs();
    }
}

#[derive(Clone)]
pub enum KObject {
    RGate(SRc<RGateObject>),
    SGate(SRc<SGateObject>),
    MGate(SRc<MGateObject>),
    Map(SRc<MapObject>),
    Serv(SRc<ServObject>),
    Sess(SRc<SessObject>),
    Sem(SRc<SemObject>),
    Activity(Rc<Activity>),
    KMem(SRc<KMemObject>),
    Tile(SRc<TileObject>),
    EP(Rc<EPObject>),
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
    kobj_size::<TileObject>() + kobj_size::<EPQuota>(),
    kobj_size::<EPObject>(),
];

impl KObject {
    pub fn size(&self) -> usize {
        // get the index in the enum
        let idx: usize = unsafe { *(self as *const _ as *const usize) };
        KOBJ_SIZES[idx]
    }
}

impl fmt::Debug for KObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KObject::SGate(s) => write!(f, "{:?}", s),
            KObject::RGate(r) => write!(f, "{:?}", r),
            KObject::MGate(m) => write!(f, "{:?}", m),
            KObject::Map(m) => write!(f, "{:?}", m),
            KObject::Serv(s) => write!(f, "{:?}", s),
            KObject::Sess(s) => write!(f, "{:?}", s),
            KObject::Activity(v) => write!(f, "{:?}", v),
            KObject::Sem(s) => write!(f, "{:?}", s),
            KObject::KMem(k) => write!(f, "{:?}", k),
            KObject::Tile(p) => write!(f, "{:?}", p),
            KObject::EP(e) => write!(f, "{:?}", e),
        }
    }
}

pub struct GateEP {
    ep: Weak<EPObject>,
}

impl GateEP {
    fn new() -> Self {
        Self { ep: Weak::new() }
    }

    pub fn get_ep(&self) -> Option<Rc<EPObject>> {
        self.ep.upgrade()
    }

    pub fn set_ep(&mut self, o: &Rc<EPObject>) {
        self.ep = Rc::downgrade(o);
    }

    pub fn remove_ep(&mut self) {
        self.ep = Weak::new()
    }
}

pub enum GateObject {
    Recv(SRc<RGateObject>),
    Send(SRc<SGateObject>),
    Mem(SRc<MGateObject>),
}

pub struct BaseGate {
    gep: RefCell<GateEP>,
}

impl BaseGate {
    pub fn set_ep(&self, ep: &Rc<EPObject>, gobj: GateObject) {
        self.gep.borrow_mut().set_ep(ep);
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
    pub fn new(order: u32, msg_order: u32, serial: bool) -> SRc<Self> {
        SRc::new(Self {
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
    rgate: SRc<RGateObject>,
    label: Label,
    credits: u32,
}

impl SGateObject {
    pub fn new(rgate: &SRc<RGateObject>, label: Label, credits: u32) -> SRc<Self> {
        SRc::new(Self {
            base: BaseGate::default(),
            rgate: rgate.clone(),
            label,
            credits,
        })
    }

    pub fn rgate(&self) -> &SRc<RGateObject> {
        &self.rgate
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
            if let Some((recv_tile, recv_ep)) = self.rgate().location() {
                let tilemux = tilemng::tilemux(sep.tile_id());
                tilemux
                    .invalidate_reply_eps(recv_tile, recv_ep, sep.ep())
                    .unwrap();
            }
        }
    }
}

impl Deref for SGateObject {
    type Target = BaseGate;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl fmt::Debug for SGateObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SGate[rgate=")?;
        self.rgate.print_loc(f)?;
        write!(f, ", lbl={:#x}, crd={}]", self.label, self.credits)
    }
}

pub struct MGateObject {
    base: BaseGate,
    mem: mem::Allocation,
    perms: kif::Perm,
    derived: bool,
}

impl MGateObject {
    pub fn new(mem: mem::Allocation, perms: kif::Perm, derived: bool) -> SRc<Self> {
        SRc::new(Self {
            base: BaseGate::default(),
            mem,
            perms,
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
}

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
    serv: SRc<Service>,
    owner: bool,
    creator: usize,
}

impl ServObject {
    pub fn new(serv: SRc<Service>, owner: bool, creator: usize) -> SRc<Self> {
        SRc::new(Self {
            serv,
            owner,
            creator,
        })
    }

    pub fn service(&self) -> &SRc<Service> {
        &self.serv
    }

    pub fn creator(&self) -> usize {
        self.creator
    }
}

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
    srv: SRc<ServObject>,
    creator: usize,
    ident: u64,
    pub auto_close: bool,
}

impl SessObject {
    pub fn new(srv: &SRc<ServObject>, creator: usize, ident: u64, auto_close: bool) -> SRc<Self> {
        SRc::new(Self {
            srv: srv.clone(),
            creator,
            ident,
            auto_close,
        })
    }

    pub fn service(&self) -> &SRc<ServObject> {
        &self.srv
    }

    pub fn creator(&self) -> usize {
        self.creator
    }

    pub fn ident(&self) -> u64 {
        self.ident
    }

    pub fn close_async(sess: KObjectOwnedRef<Self>, revoker: ActId) {
        if sess.auto_close {
            // don't send the close, if the server is the revoker
            if sess.srv.service().activity().id() == revoker {
                return;
            }

            log!(
                LogFlags::KernServ,
                "Sending close(sess={:#x}) to service {} with creator {}",
                sess.ident(),
                sess.srv.service().name(),
                sess.creator,
            );

            let mut smsg = MsgBuf::borrow_def();
            build_vmsg!(smsg, service::Request::Close { sid: sess.ident });

            // this should never fail, because the close request fails only if the creator does not
            // own the session. but we know here that the creator owns this session.
            let srv = KObjectOwnedRef::new(sess.srv.clone());
            let creator = sess.creator as Label;
            drop(sess);

            Service::send_receive_async(srv, creator, smsg).unwrap();
        }
    }
}

impl fmt::Debug for SessObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Sess[service={}, creator={}, ident={:#x}]",
            self.service().service().name(),
            self.creator,
            self.ident,
        )
    }
}

pub struct SemObject {
    counter: Cell<u32>,
    waiters: Cell<i32>,
}

impl SemObject {
    pub fn new(counter: u32) -> SRc<Self> {
        SRc::new(Self {
            counter: Cell::from(counter),
            waiters: Cell::from(0),
        })
    }

    pub fn down_async(s: KObjectOwnedRef<Self>) -> Result<(), Error> {
        let sem_weak = s.downgrade();
        loop {
            let sem = sem_weak
                .upgrade()
                .ok_or_else(|| Error::new(Code::NotFound))?;
            if sem.counter.get() != 0 {
                sem.counter.set(sem.counter.get() - 1);
                break;
            }

            sem.waiters.set(sem.waiters.get() + 1);
            let event = sem.get_event();
            let tmp_weak = sem.downgrade();

            wait_for_async(event);

            let sem = tmp_weak
                .upgrade()
                .ok_or_else(|| Error::new(Code::NotFound))?;
            if sem.waiters.get() == -1 {
                return Err(Error::new(Code::RecvGone));
            }
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
        self.waiters.set(-1);
    }

    fn get_event(&self) -> thread::Event {
        self as *const Self as thread::Event
    }
}

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

pub struct EPQuota {
    id: QuotaId,
    total: Cell<usize>,
    left: Cell<usize>,
}

impl EPQuota {
    pub fn new(eps: usize) -> Rc<Self> {
        static NEXT_ID: StaticCell<QuotaId> = StaticCell::new(0);
        let id = NEXT_ID.get();
        NEXT_ID.set(id + 1);

        Rc::new(Self {
            id,
            total: Cell::from(eps),
            left: Cell::from(eps),
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
}

pub struct TileObject {
    tile: TileId,
    cur_acts: Cell<u32>,
    ep_quota: Rc<EPQuota>,
    time_quota: QuotaId,
    pt_quota: QuotaId,
    derived: bool,
}

impl TileObject {
    pub fn new(
        tile: TileId,
        ep_quota: Rc<EPQuota>,
        time_quota: QuotaId,
        pt_quota: QuotaId,
        derived: bool,
    ) -> SRc<Self> {
        let res = SRc::new(Self {
            tile,
            cur_acts: Cell::from(0),
            ep_quota: ep_quota.clone(),
            time_quota,
            pt_quota,
            derived,
        });
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: {} new TileObject with EPs={}, time={}, pts={}",
            tile,
            &*res as *const _ as usize,
            if derived { "derived" } else { "created" },
            ep_quota.total(),
            time_quota,
            pt_quota,
        );
        res
    }

    pub fn tile(&self) -> TileId {
        self.tile
    }

    pub fn derived(&self) -> bool {
        self.derived
    }

    pub fn activities(&self) -> u32 {
        self.cur_acts.get()
    }

    pub fn ep_quota(&self) -> &Rc<EPQuota> {
        &self.ep_quota
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

    pub fn add_activity(&self) {
        self.cur_acts.set(self.activities() + 1);
    }

    pub fn rem_activity(&self) {
        assert!(self.activities() > 0);
        self.cur_acts.set(self.activities() - 1);
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
                Err(e) if e.code() == Code::NoMEP => mem::Allocation::new(
                    GlobAddr::new_with(self.tile(), desc.mem_offset() as GlobOff),
                    desc.mem_size() as GlobOff,
                ),
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

    pub fn alloc(&self, eps: usize) {
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: allocating {} EPs ({} left)",
            self.tile,
            self as *const _ as usize,
            eps,
            self.ep_quota.left()
        );
        assert!(self.ep_quota.left() >= eps);
        self.ep_quota.left.set(self.ep_quota.left() - eps);
    }

    pub fn free(&self, eps: usize) {
        assert!(self.ep_quota.left() + eps <= self.ep_quota.total());
        self.ep_quota.left.set(self.ep_quota.left() + eps);
        log!(
            LogFlags::KernTiles,
            "Tile[{}, {:#x}]: freed {} EPs ({} left)",
            self.tile,
            self as *const _ as usize,
            eps,
            self.ep_quota.left()
        );
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

    pub fn revoke_async(tile: KObjectOwnedRef<Self>, parent: &TileObject) {
        // we free the EP quota if it's different from our parent's quota (only our own childs can
        // have the same EP quota, but they are already gone).
        if !Rc::ptr_eq(&tile.ep_quota, &parent.ep_quota) {
            // grant the EPs back to our parent
            parent.free(tile.ep_quota.left());
            assert!(tile.ep_quota.left() == tile.ep_quota.total());
        }

        // same for time and pts: free the ones that are different
        let time = if tile.time_quota != parent.time_quota {
            Some(tile.time_quota)
        }
        else {
            None
        };
        let pts = if tile.pt_quota != parent.pt_quota {
            Some(tile.pt_quota)
        }
        else {
            None
        };
        if time.is_some() || pts.is_some() {
            let tile_id = tile.tile();
            drop(tile);

            TileMux::remove_quotas_async(tilemng::tilemux(tile_id), time, pts).ok();
        }
    }
}

impl fmt::Debug for TileObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tile[id={}, eps={}, actitivies={}, derived={}]",
            self.tile,
            self.ep_quota.left(),
            self.activities(),
            self.derived,
        )
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum EPCategory {
    PMP,
    Std,
    Custom,
}

pub struct EPObject {
    cat: EPCategory,
    gate: RefCell<Option<GateObject>>,
    act: Weak<Activity>,
    ep: EpId,
    replies: usize,
    tile: SRc<TileObject>,
}

impl EPObject {
    pub fn new(
        cat: EPCategory,
        act: Weak<Activity>,
        ep: EpId,
        replies: usize,
        tile: &SRc<TileObject>,
    ) -> Rc<Self> {
        let maybe_act = act.upgrade();
        let ep = Rc::new(Self {
            cat,
            gate: RefCell::from(None),
            act,
            ep,
            replies,
            tile: tile.clone(),
        });
        if let Some(v) = maybe_act {
            v.add_ep(ep.clone());
        }
        ep
    }

    pub fn tile_id(&self) -> TileId {
        self.tile.tile()
    }

    // TODO don't hand out Rc<Activity>, but KObjectOwnedRef<...>
    pub fn activity(&self) -> Option<Rc<Activity>> {
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

    pub fn revoke(ep: &Rc<Self>) {
        if let Some(v) = ep.act.upgrade() {
            v.rem_ep(ep);
        }
    }

    pub fn is_configured(&self) -> bool {
        self.gate.borrow().is_some()
    }

    pub fn deconfigure(&self, force: bool) -> Result<bool, Error> {
        let mut invalidated = false;
        if let Some(ref gate) = self.gate.borrow_mut().take() {
            let tile_id = self.tile_id();

            // invalidate receive and send EPs
            match gate {
                GateObject::Recv(_) | GateObject::Send(_) => {
                    tilemng::tilemux(tile_id).invalidate_ep(
                        self.activity().unwrap().id(),
                        self.ep,
                        force,
                        true,
                    )?;
                    invalidated = true;
                },
                _ => {},
            }

            match gate {
                // invalidate reply EPs
                GateObject::Send(s) => s.invalidate_reply_eps(),
                // deactivate receive gate
                GateObject::Recv(r) => r.deactivate(),
                _ => {},
            }

            // we tell the gate that it's ep is no longer valid
            match gate {
                GateObject::Recv(g) => g.gep.borrow_mut().remove_ep(),
                GateObject::Send(g) => g.gep.borrow_mut().remove_ep(),
                GateObject::Mem(g) => g.gep.borrow_mut().remove_ep(),
            }
        }
        Ok(invalidated)
    }
}

impl Drop for EPObject {
    fn drop(&mut self) {
        if self.cat == EPCategory::Custom {
            tilemng::tilemux(self.tile.tile).free_eps(self.ep, 1 + self.replies);

            self.tile.free(1 + self.replies);
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
            self.tile
        )
    }
}

pub struct KMemObject {
    id: QuotaId,
    quota: usize,
    left: Cell<usize>,
}

impl KMemObject {
    pub fn new(quota: usize) -> SRc<Self> {
        static NEXT_ID: StaticCell<QuotaId> = StaticCell::new(0);
        let id = NEXT_ID.get();
        NEXT_ID.set(id + 1);

        let kmem = SRc::new(Self {
            id,
            quota,
            left: Cell::from(quota),
        });
        log!(LogFlags::KernKMem, "{:?} created", kmem);
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

    pub fn revoke(&self, act: &Activity, sel: kif::CapSel, parent: &KMemObject) {
        // grant the kernel memory back to our parent
        parent.free(act, sel, self.left());
        assert!(self.left() == self.quota);
    }
}

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
        assert!(self.left() == self.quota);
    }
}

pub struct MapObject {
    glob: Cell<GlobAddr>,
    flags: Cell<kif::PageFlags>,
    mapped: Cell<bool>,
}

impl MapObject {
    pub fn new(glob: GlobAddr, flags: kif::PageFlags) -> SRc<Self> {
        SRc::new(Self {
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
        map: KObjectOwnedRef<Self>,
        act_id: ActId,
        act_tile: TileId,
        virt: VirtAddr,
        glob: GlobAddr,
        pages: usize,
        flags: kif::PageFlags,
    ) -> Result<(), Error> {
        let map_weak = map.downgrade();
        TileMux::map_async(tilemng::tilemux(act_tile), act_id, virt, glob, pages, flags).map(|_| {
            if let Some(map) = map_weak.upgrade() {
                map.glob.replace(glob);
                map.flags.replace(flags);
                map.mapped.set(true);
            }
        })
    }

    pub fn unmap_async(act: &Activity, virt: VirtAddr, pages: usize) {
        // TODO currently, it can happen that we've already stopped the activity, but still
        // accept/continue a syscall that inserts something into the activity's table.
        if act.state() != State::DEAD {
            TileMux::unmap_async(tilemng::tilemux(act.tile_id()), act.id(), virt, pages).ok();
        }
    }
}

impl fmt::Debug for MapObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Map[glob={}, flags={:#x}]", self.global(), self.flags())
    }
}
