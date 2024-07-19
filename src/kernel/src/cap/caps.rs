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

use base::cell::{RefCell, RefMut, StaticCell};
use base::cfg;
use base::col::Treap;
use base::errors::{Code, Error, VerboseError};
use base::io::LogFlags;
use base::kif::{CapRngDesc, CapSel};
use base::log;
use base::mem::{size_of, GlobOff, VirtAddr};
use base::tcu::ActId;
use core::cmp;
use core::fmt;
use core::ptr::NonNull;

use thread::AsyncRc;

use crate::cap::{EPObject, GateEP, KObject, MapObject, SessObject, TileObject};
use crate::ktcu;
use crate::tiles::{tilemng, Activity, State, INVAL_ID};

use super::IntoKObject;

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct SelRange {
    start: CapSel,
    count: CapSel,
}

impl SelRange {
    pub fn new(sel: CapSel) -> Self {
        Self::new_range(sel, 1)
    }

    pub fn new_range(sel: CapSel, count: CapSel) -> Self {
        SelRange { start: sel, count }
    }
}

impl fmt::Debug for SelRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.start)
    }
}

impl cmp::PartialOrd for SelRange {
    fn partial_cmp(&self, other: &SelRange) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl cmp::Ord for SelRange {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if self.start >= other.start && self.start < other.start + other.count {
            cmp::Ordering::Equal
        }
        else if self.start < other.start {
            cmp::Ordering::Less
        }
        else {
            cmp::Ordering::Greater
        }
    }
}

pub struct CapTable {
    caps: Treap<SelRange, Capability>,
    act: Option<NonNull<Activity>>,
}

unsafe fn as_shared<T>(obj: &mut T) -> NonNull<T> {
    NonNull::new_unchecked(obj as *mut T)
}

impl Default for CapTable {
    fn default() -> Self {
        Self {
            caps: Treap::new(),
            act: None,
        }
    }
}

impl CapTable {
    fn activity(&self) -> &Activity {
        unsafe { &(*self.act.unwrap().as_ptr()) }
    }

    pub fn set_activity(&mut self, act: &Activity) {
        let act_ptr = unsafe { NonNull::new_unchecked(act as *const _ as *mut _) };
        self.act = Some(act_ptr);
    }

    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }

    pub fn unused(&self, sel: CapSel) -> bool {
        self.caps.get(&SelRange::new(sel)).is_none()
    }

    pub fn range_unused(&self, crd: &CapRngDesc) -> bool {
        for s in crd.start()..crd.start() + crd.count() {
            if !self.unused(s) {
                return false;
            }
        }
        true
    }

    pub fn get(&self, sel: CapSel) -> Result<&Capability, Error> {
        self.caps
            .get(&SelRange::new(sel))
            .ok_or_else(|| Error::new(Code::InvCap))
    }

    pub fn get_mut(&mut self, sel: CapSel) -> Result<&mut Capability, Error> {
        self.caps
            .get_mut(&SelRange::new(sel))
            .ok_or_else(|| Error::new(Code::InvCap))
    }

    pub fn get_kobj<T>(&self, sel: CapSel) -> Result<T, VerboseError>
    where
        T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
    {
        self.get(sel)?.get()
    }

    #[inline(always)]
    pub fn insert(&mut self, cap: Capability) -> Result<(), Error> {
        self.insert_new(cap, None)
    }

    #[inline(always)]
    pub fn insert_as_child(&mut self, cap: Capability, parent_sel: CapSel) -> Result<(), Error> {
        unsafe {
            let parent = self.get_shared(parent_sel);
            self.insert_new(cap, parent)
        }
    }

    #[inline(always)]
    pub fn insert_as_child_from(
        &mut self,
        cap: Capability,
        mut par_tbl: RefMut<'_, CapTable>,
        par_sel: CapSel,
    ) -> Result<(), Error> {
        unsafe {
            let parent = par_tbl.get_shared(par_sel);
            self.insert_new(cap, parent)
        }
    }

    #[inline(always)]
    unsafe fn get_shared(&mut self, sel: CapSel) -> Option<NonNull<Capability>> {
        self.caps
            .get_mut(&SelRange::new(sel))
            .map(|cap| NonNull::new_unchecked(cap))
    }

    #[inline(always)]
    fn insert_new(
        &mut self,
        cap: Capability,
        parent: Option<NonNull<Capability>>,
    ) -> Result<(), Error> {
        if self.caps.get(cap.sel_range()).is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        let act = self.activity();
        if !act
            .kmem()
            .unwrap()
            .alloc(act, cap.sel(), cap.obj.size() + Capability::size())
        {
            return Err(Error::new(Code::NoSpace));
        }

        unsafe {
            let child_cap = self.do_insert(cap);
            if let Some(parent) = parent {
                (*parent.as_ptr()).inherit(child_cap);
            }
            log!(LogFlags::KernCaps, "Creating cap {:?}", child_cap);
        }
        Ok(())
    }

    pub fn obtain(&mut self, sel: CapSel, cap: &mut Capability, child: bool) -> Result<(), Error> {
        let mut nc: Capability = (*cap).clone();
        nc.sels = SelRange::new(sel);
        nc.derived = true;

        if self.caps.get(nc.sel_range()).is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        let act = self.activity();
        if !act.kmem().unwrap().alloc(act, sel, Capability::size()) {
            return Err(Error::new(Code::NoSpace));
        }

        let nc = self.do_insert(nc);
        log!(LogFlags::KernCaps, "Cloning cap {:?}", nc);
        if child {
            cap.inherit(nc);
        }
        else {
            nc.inherit(cap);
        }
        Ok(())
    }

    fn do_insert(&mut self, mut cap: Capability) -> &mut Capability {
        unsafe {
            cap.table = Some(as_shared(self));
        }
        self.caps.insert(*cap.sel_range(), cap)
    }

    pub fn revoke_async(
        tbl: &RefCell<Self>,
        crd: CapRngDesc,
        own: bool,
        revoker: ActId,
    ) -> Result<(), Error> {
        let mut sel = crd.start();
        while sel < crd.start() + crd.count() {
            let tbl_ref = tbl.borrow_mut();
            match RefMut::filter_map(tbl_ref, |t| t.get_mut(sel).ok()) {
                Ok(cap) => {
                    if !cap.can_revoke() {
                        return Err(Error::new(Code::NotRevocable));
                    }

                    let len = cap.len();
                    if Capability::revoke_single_async(cap, own, revoker) {
                        sel += len;
                    }
                },

                Err(_tbl) => {
                    sel += 1;
                },
            }
        }
        Ok(())
    }

    pub fn revoke_all_async(tbl: &RefCell<Self>, revoker: ActId) {
        loop {
            let tbl_ref = tbl.borrow_mut();
            match RefMut::filter_map(tbl_ref, |t| t.caps.get_root_mut()) {
                Ok(cap) => {
                    Capability::revoke_single_async(cap, true, revoker);
                },
                Err(_tbl) => break,
            }
        }
    }
}

impl fmt::Debug for CapTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapTable[\n{:?}]", self.caps)
    }
}

#[derive(Clone)]
pub struct Capability {
    sels: SelRange,
    obj: KObject,
    table: Option<NonNull<CapTable>>,
    child: Option<NonNull<Capability>>,
    parent: Option<NonNull<Capability>>,
    next: Option<NonNull<Capability>>,
    prev: Option<NonNull<Capability>>,
    derived: bool,
}

impl Capability {
    const fn size() -> usize {
        base::const_assert!(size_of::<Capability>() <= 128);
        128 + crate::slab::HEADER_SIZE
    }

    pub fn new<T>(sel: CapSel, obj: AsyncRc<T>) -> Self
    where
        AsyncRc<T>: IntoKObject<T>,
    {
        Self::new_range(SelRange::new(sel), obj)
    }

    pub fn new_range<T>(sels: SelRange, obj: AsyncRc<T>) -> Self
    where
        AsyncRc<T>: IntoKObject<T>,
    {
        Capability {
            sels,
            // safety: as we directly keep the KObject in the capability, the conversion is okay
            obj: unsafe { obj.into_kobj() },
            table: None,
            child: None,
            parent: None,
            next: None,
            prev: None,
            derived: false,
        }
    }

    pub fn sel_range(&self) -> &SelRange {
        &self.sels
    }

    pub fn sel(&self) -> CapSel {
        self.sels.start
    }

    pub fn len(&self) -> CapSel {
        self.sels.count
    }

    pub fn get<T>(&self) -> Result<T, VerboseError>
    where
        T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
    {
        // safety: we directly turn it into a KObjectOwnedRef here, so that it's okay
        unsafe { self.get_unchecked() }.try_into()
    }

    //
    // # Safety
    //
    // The caller cannot keep the KObject across async calls.
    pub unsafe fn get_unchecked(&self) -> &KObject {
        &self.obj
    }

    pub fn has_parent(&self) -> bool {
        self.parent.is_some()
    }

    pub fn get_root(&mut self) -> &mut Capability {
        if let Some(mut cap) = self.parent {
            unsafe {
                while let Some(p) = (*cap.as_ptr()).parent {
                    cap = p;
                }
                &mut *cap.as_ptr()
            }
        }
        else {
            self
        }
    }

    pub fn find_child<P>(&mut self, pred: P) -> Option<&mut Capability>
    where
        P: Fn(&Capability) -> bool,
    {
        let mut next = self.child;
        while let Some(n) = next {
            unsafe {
                if pred(&*n.as_ptr()) {
                    return Some(&mut *n.as_ptr());
                }
                next = (*n.as_ptr()).next;
            }
        }
        None
    }

    fn inherit(&mut self, child: &mut Capability) {
        unsafe {
            child.parent = Some(as_shared(self));
            child.child = None;
            child.next = self.child;
            child.prev = None;
            if let Some(n) = child.next {
                (*n.as_ptr()).prev = Some(as_shared(child));
            }
            self.child = Some(as_shared(child));
        }
    }

    /// Revoke a single leaf capability in the derivation tree of `self`.
    ///
    /// Returns `true` when no more capabilities are found.
    fn revoke_single_async(mut cap: RefMut<'_, Self>, self_included: bool, revoker: ActId) -> bool {
        let mut is_child = false;
        // Loop to the first child.
        loop {
            match RefMut::filter_map(cap, |c| c.child.as_mut()) {
                // SAFETY: This should be safe since no other thread must
                // currently hold a reference inside the RefCell of a
                // capability table of any application.
                // Additionally, all capability references should always be
                // valid when switching threads.
                Ok(child) => unsafe {
                    is_child = true;
                    cap = RefMut::map(child, |c| c.as_mut())
                },
                Err(c) => {
                    cap = c;
                    break;
                },
            }
        }

        if !self_included && !is_child {
            return true;
        }

        log!(LogFlags::KernCaps, "Revoking cap {:?}", *cap);

        // Unlink cap from derivation tree.
        // SAFETY: All references in the derivation tree must be valid.
        unsafe {
            if let Some(n) = cap.next {
                (*n.as_ptr()).prev = cap.prev;
            }
            if let Some(p) = cap.prev {
                (*p.as_ptr()).next = cap.next;
            }
            if let Some(p) = cap.parent {
                if cap.prev.is_none() {
                    let child = &mut (*p.as_ptr()).child;
                    *child = cap.next;
                }
            }
            // cap is a leaf and has no child.
        }

        // Remove cap from table.
        let mut tbl = cap.table.unwrap();
        let sel = cap.sel();
        drop(cap);
        // SAFETY: The reference to the cap table should be valid as long as
        // the cap is reachable.
        // No one should hold a reference to any cap table (content) currently.
        // (We dropped our cap just above.)
        let tbl = unsafe { tbl.as_mut() };
        // Move the very cap we just dropped out of its cap table.
        let cap = tbl.caps.remove(&SelRange::new(sel)).unwrap();

        // Release the cap that is now completely unreachable.
        cap.release_async(revoker);

        !is_child
    }

    fn table(&self) -> &CapTable {
        unsafe { &*self.table.unwrap().as_ptr() }
    }

    #[allow(dead_code)]
    fn table_mut(&mut self) -> &mut CapTable {
        unsafe { &mut *self.table.unwrap().as_ptr() }
    }

    fn activity(&self) -> &Activity {
        self.table().activity()
    }

    fn invalidate_ep(mut cgp: RefMut<'_, GateEP>, revoker: ActId, notify: bool) {
        if let Some(ep) = cgp.get_ep() {
            let mut tilemux = tilemng::tilemux(ep.tile_id());
            if let Some(act) = ep.activity() {
                // if that fails, just ignore it
                tilemux
                    .invalidate_ep(act.id(), ep.ep(), !ep.is_rgate(), true)
                    .ok();

                // notify TileMux about the invalidation if it's not a self-invalidation
                if notify && revoker != act.id() {
                    tilemux.notify_invalidate(act.id(), ep.ep()).ok();
                }
            }
            else {
                // force invalidate without activity (no notification etc.)
                ktcu::invalidate_ep_remote(ep.tile_id(), ep.ep(), true).ok();
            }

            EPObject::revoke(ep);

            cgp.remove_ep();
        }
    }

    fn can_revoke(&self) -> bool {
        match self.obj {
            KObject::KMem(ref k) => k.left() == k.quota(),
            KObject::Tile(ref tile) => tile.activities() == 0,
            _ => true,
        }
    }

    fn release_async(self, revoker: ActId) {
        log!(LogFlags::KernCaps, "Freeing cap {:?}", self);

        let act = self.activity();
        let sel = self.sel();
        if let Some(kmem) = act.kmem() {
            if !self.derived {
                // if it's not derived, we created the cap and thus will also free the kobject
                kmem.free(act, sel, Capability::size() + self.obj.size());
            }
            else {
                // give quota for cap back in every case
                kmem.free(act, sel, Capability::size());
            }
        }

        if !self.derived {
            assert_eq!(self.obj.ref_count(), 1);
        }

        match self.obj {
            KObject::Activity(v) => {
                if !self.derived {
                    v.revoke_caps_async(revoker);
                }
            },

            KObject::EP(e) => {
                EPObject::revoke(AsyncRc::new(e.clone()));
            },

            KObject::Tile(tile) => {
                if !self.derived {
                    if let Some(parent) = self.parent {
                        let parent = unsafe { &(*parent.as_ptr()) };
                        // TODO we cannot use these references across the async call below
                        let tileobj = unsafe { parent.get_unchecked().clone() };
                        if let KObject::Tile(p) = tileobj {
                            TileObject::revoke_async(AsyncRc::new(tile), &p);
                        }
                    }
                }
            },

            KObject::KMem(k) => {
                if !self.derived {
                    if let Some(parent) = self.parent {
                        let parent = unsafe { &(*parent.as_ptr()) };
                        // TODO we cannot use these references across the async call below
                        let kmemobj = unsafe { parent.get_unchecked().clone() };
                        if let KObject::KMem(p) = kmemobj {
                            k.revoke(parent.activity(), parent.sel(), &p);
                        }
                    }
                }
            },

            KObject::SGate(o) => {
                o.invalidate_reply_eps();
                Self::invalidate_ep(o.gate_ep_mut(), revoker, true);
            },

            KObject::RGate(o) => {
                Self::invalidate_ep(o.gate_ep_mut(), INVAL_ID, false);
                // notify potential send-gate activations blocked on this receive gate
                thread::notify(o.get_event(), None);
            },

            KObject::MGate(o) => {
                Self::invalidate_ep(o.gate_ep_mut(), INVAL_ID, false);
            },

            KObject::Serv(s) => {
                s.abort();
            },

            KObject::Sess(s) => {
                // if the session is derived, we notify the server about this and let him remove the
                // session. this has the consequence that in delegation chains every activity in the
                // chain can remove the session for all. I think this is fine, because we are never
                // sharing a session between multiple activities, but are at most "granting" the
                // session to someone else if we don't want to use it ourself.
                if self.derived {
                    // release the Rc within the KObject before doing the async call, because the
                    // server typically revokes its non-derived cap during the async call. That is,
                    // without releasing our reference the strong-count check below fails.
                    SessObject::close_async(AsyncRc::new(s), revoker);
                }
            },

            KObject::Map(ref m) => {
                // TODO currently, it can happen that we've already stopped the activity, but still
                // accept/continue a syscall that inserts something into the activity's table.
                if m.mapped() && act.state() != State::DEAD {
                    let virt = VirtAddr::new((sel as GlobOff) << cfg::PAGE_BITS);
                    MapObject::unmap_async(act.id(), act.tile_id(), virt, self.len() as usize);
                }
            },

            KObject::Sem(s) => {
                s.revoke();
            },
        }
    }
}

fn print_childs(cap: NonNull<Capability>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    static LAYER: StaticCell<u32> = StaticCell::new(5);
    use core::fmt::Write;
    let mut next = Some(cap);
    loop {
        match next {
            None => return Ok(()),
            Some(n) => unsafe {
                f.write_char('\n')?;
                for _ in 0..LAYER.get() {
                    f.write_char(' ')?;
                }
                LAYER.set(LAYER.get() + 1);
                write!(f, "=> {:?}", *n.as_ptr())?;
                LAYER.set(LAYER.get() - 1);

                next = (*n.as_ptr()).next;
            },
        }
    }
}

impl fmt::Debug for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cap[act={}, sel={}, len={}: {:?}]",
            self.activity().id(),
            self.sel(),
            self.len(),
            self.obj
        )?;
        if let Some(c) = self.child {
            print_childs(c, f)?;
        }
        Ok(())
    }
}
