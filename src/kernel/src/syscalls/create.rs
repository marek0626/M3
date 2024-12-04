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

use base::build_vmsg;
use base::col::ToString;
use base::errors::Code;
use base::kif::INVALID_SEL;
use base::kif::{syscalls, CapRngDesc, CapSel, CapType, PageFlags, Perm};
use base::mem::{GlobAddr, GlobOff, MsgBuf, VirtAddr, VirtAddrRaw};
use base::{cfg, kif};
use base::{format, tcu};

use thread::{Downgradable, TempRc, Upgradable};

use crate::cap::{
    Capability, EPCategory, EPObject, KMemObject, MGateObject, MapObject, RGateObject, SGateObject,
    SelRange, SemObject, ServObject, SessObject, TileObject,
};
use crate::com::Service;
use crate::mem;
use crate::platform;
use crate::syscalls::{get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, ActivityFlags, ActivityMng};
use crate::{kerrno, kerror};

#[inline(never)]
pub fn create_mgate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateMGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "create_mgate(dst={}, act={}, addr={}, size={:#x}, perms={:?})",
        r.dst,
        r.act,
        r.addr,
        r.size,
        r.perms,
    );

    if (r.addr.as_goff() & cfg::PAGE_MASK as GlobOff) != 0
        || (r.size & cfg::PAGE_MASK as GlobOff) != 0
    {
        return Err(kerrno(Code::InvArgs).context("Virt address and size need to be page-aligned"));
    }

    let tgt_act: TempRc<Activity> = act.get_kobj(r.act)?;

    let sel = (r.addr.as_goff() / cfg::PAGE_SIZE as GlobOff) as CapSel;
    let glob = if platform::tile_desc(tgt_act.tile_id()).has_virtmem() {
        let pages = (r.size / cfg::PAGE_SIZE as GlobOff) as CapSel;
        if pages == 0 {
            return Err(kerrno(Code::InvArgs).context("Region is empty"));
        }

        let map_caps = tgt_act.map_caps().borrow();
        let map_cap = map_caps.get(sel)?;
        let map_obj: TempRc<MapObject> = map_cap.get()?;

        // TODO think about the flags in MapObject again
        let map_perms = Perm::from_bits_truncate(map_obj.flags().bits() as u32);
        if !(r.perms & !Perm::RWX).is_empty() || !(r.perms & !map_perms).is_empty() {
            return Err(kerrno(Code::NoPerm).context("Invalid permissions"));
        }

        let pages = (r.size / cfg::PAGE_SIZE as GlobOff) as CapSel;
        let off = sel - map_cap.sel();
        if off + pages > map_cap.len() {
            return Err(kerrno(Code::InvArgs).context("Invalid length"));
        }

        let phys =
            crate::ktcu::glob_to_phys_remote(tgt_act.tile_id(), map_obj.global(), map_obj.flags())?;
        GlobAddr::new_with(tgt_act.tile_id(), phys.as_goff())
    }
    else {
        if r.size == 0 {
            return Err(kerrno(Code::InvArgs).context("Region is empty"));
        }
        // use the same error code here as above where we fail with InvCap for non-existing mapping
        // capabilities (unmapped regions)
        if r.addr + r.size >= cfg::MEM_CAP_END {
            return Err(kerrno(Code::InvCap).context("Region is out of bounds"));
        }

        GlobAddr::new_with(tgt_act.tile_id(), r.addr.as_goff())
    };

    let mem = mem::Allocation::new(glob, r.size);
    let mgate = MGateObject::new(mem, r.perms, true);
    let cap = Capability::new(r.dst, mgate);

    if platform::tile_desc(tgt_act.tile_id()).has_virtmem() {
        let map_caps = tgt_act.map_caps().borrow_mut();
        act.obj_caps()
            .borrow_mut()
            .insert_as_child_from(cap, map_caps, sel)?;
    }
    else {
        act.obj_caps().borrow_mut().insert_as_child(cap, r.act)?;
    }

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_rgate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateRGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "create_rgate(dst={}, size={:#x}, msg_size={:#x})",
        r.dst,
        1u32.checked_shl(r.order).unwrap_or(0),
        1u32.checked_shl(r.msg_order).unwrap_or(0)
    );

    let mut act_caps = act.obj_caps().borrow_mut();

    if r.msg_order.checked_add(r.order).is_none()
        || r.msg_order > r.order
        || r.order - r.msg_order >= 32
        || (1 << (r.order - r.msg_order)) > cfg::MAX_RB_SIZE
    {
        return Err(kerrno(Code::InvArgs).context("Invalid size"));
    }

    let rgate = RGateObject::new(r.order, r.msg_order, false);
    act_caps.insert(Capability::new(r.dst, rgate))?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_sgate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateSGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "create_sgate(dst={}, rgate={}, label={:#x}, credits={})",
        r.dst,
        r.rgate,
        r.label,
        r.credits
    );

    let mut act_caps = act.obj_caps().borrow_mut();

    let cap = {
        let rgate: TempRc<RGateObject> = act_caps.get_kobj(r.rgate)?;
        let sgate = SGateObject::new(rgate.downgrade_store(), r.label, r.credits);
        Capability::new(r.dst, sgate)
    };

    act_caps.insert_as_child(cap, r.rgate)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_srv(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateSrv<'_> = get_request(&msg)?;

    sysc_log!(
        act,
        "create_srv(dst={}, rgate={}, creator={}, name={})",
        r.dst,
        r.rgate,
        r.creator,
        r.name
    );

    if r.name.is_empty() {
        return Err(kerrno(Code::InvArgs).context("Invalid server name"));
    }

    let mut act_caps = act.obj_caps().borrow_mut();

    let cap = {
        let rgate: TempRc<RGateObject> = act_caps.get_kobj(r.rgate)?;
        if !rgate.activated() {
            return Err(kerrno(Code::InvArgs).context("RGate is not activated"));
        }

        let serv = Service::new(act.clone(), r.name.to_string(), rgate);
        let serv_obj = ServObject::new(serv, true, r.creator);
        Capability::new(r.dst, serv_obj)
    };

    act_caps.insert(cap)?;

    drop(msg);
    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_sess(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateSess = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "create_sess(dst={}, srv={}, creator={}, ident={:#x}, auto_close={})",
        r.dst,
        r.srv,
        r.creator,
        r.ident,
        r.auto_close
    );

    let mut obj_caps = act.obj_caps().borrow_mut();

    let serv_cap = obj_caps.get(r.srv)?;
    // TODO maybe we should store that rather in the ServObject?
    if serv_cap.has_parent() {
        return Err(kerrno(Code::InvArgs).context("Only the service owner can create sessions"));
    }

    let serv: TempRc<ServObject> = serv_cap.get()?;
    let sess = SessObject::new(serv.downgrade_store(), r.creator, r.ident, r.auto_close);
    let cap = Capability::new(r.dst, sess);

    obj_caps.insert_as_child(cap, r.srv)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_activity_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateActivity<'_> = get_request(&msg)?;

    sysc_log!(
        act,
        "create_activity(dst={}, name={}, tile={}, kmem={})",
        r.dst,
        r.name,
        r.tile,
        r.kmem
    );

    if r.dst.count() != 3 || r.dst.cap_type() != CapType::Object {
        return Err(kerrno(Code::InvArgs).context("Invalid destination selectors"));
    }
    if r.name.is_empty() {
        return Err(kerrno(Code::InvArgs).context("Invalid name"));
    }

    let tile: TempRc<TileObject> = act.get_kobj(r.tile)?;
    if !tile.has_quota(tcu::STD_EPS_COUNT) {
        return Err(kerrno(Code::InvArgs).context(format!(
            "Tile cap has insufficient EPs (have {}, need {})",
            tile.ep_quota().left(),
            tcu::STD_EPS_COUNT
        )));
    }

    let kmem: TempRc<KMemObject> = act.get_kobj(r.kmem)?;
    // TODO kmem quota stuff

    // find contiguous space for standard EPs
    let tile_id = tile.tile();
    let tilemux = tilemng::tilemux(tile_id);
    let eps = match tilemux.find_eps(tcu::STD_EPS_COUNT) {
        Ok(eps) => eps,
        Err(e) => return Err(e.context("No free range for standard EPs")),
    };
    if tilemux.has_activities() && !platform::tile_desc(tile.tile()).has_virtmem() {
        return Err(kerrno(Code::NotSup).context("Virtual memory is required for tile sharing"));
    }
    drop(tilemux);

    let name = r.name.to_string();
    let dst_sel = r.dst.start();
    let tile_sel = r.tile;
    let kmem_sel = r.kmem;
    let act_id = act.id();
    drop(msg);

    let act_weak = act.downgrade_asyn();

    // create activity, assure that they are dropped in reverse order
    let nact = match ActivityMng::create_activity_async(
        name,
        Some((act_id, dst_sel)),
        tile,
        eps,
        kmem,
        ActivityFlags::empty(),
    ) {
        Ok(nact) => nact,
        Err(e) => return Err(e.context("Unable to create Activity")),
    };

    // TODO if something fails below we do not properly undo the steps above

    let act = match try {
        let act = try_upgrade_kobj(act_weak.clone(), INVALID_SEL)?;

        let mut parent_caps = act.obj_caps().borrow_mut();
        let mut child_caps = nact.obj_caps().borrow_mut();

        // obtain kmem and tile cap from parent
        let kmem_parent = parent_caps.get_mut(kmem_sel)?;
        child_caps.obtain(kif::SEL_KMEM, kmem_parent)?;
        let tile_parent = parent_caps.get_mut(tile_sel)?;
        child_caps.obtain(kif::SEL_TILE, tile_parent)?;

        // give activity cap to the parent and obtain it to child
        // safety: we need to keep another reference here in case the insert fails to properly destruct
        // it via Activity::stop_app_async (and not drop it immediately). that's okay, because we'll
        // get rid of the additional reference in the cancel call below
        let cap = unsafe { Capability::new_range_unchecked(SelRange::new(dst_sel), nact.clone()) };
        // inherit this cap from the kernel memory it uses to revoke it as soon as the kmem is revoked
        parent_caps.insert_as_child(cap, kmem_sel)?;
        // Do not clean activity up after inserted in capability table.

        drop(parent_caps);
        act
    } {
        Ok(a) => a,
        Err(e) => {
            // ensure that it's not dropped during the call
            let _clone = nact.clone();
            Activity::stop_app_async(TempRc::new(nact), Code::Unspecified, act_id);
            return Err(e);
        },
    };

    let mut parent_caps = act.obj_caps().borrow_mut();
    let nact: TempRc<Activity> = parent_caps.get_kobj(dst_sel).unwrap();

    let act_parent = parent_caps.get_mut(dst_sel)?;
    nact.obj_caps()
        .borrow_mut()
        .obtain(kif::SEL_ACT, act_parent)?;

    // create EP caps for the pager EPs
    if nact.tile_desc().has_virtmem() {
        let nact_weak = nact.clone().downgrade_store();
        for (i, ep) in [eps + tcu::PG_SEP_OFF, eps + tcu::PG_REP_OFF]
            .iter()
            .enumerate()
        {
            let ep = EPObject::new(
                EPCategory::Std,
                nact_weak.clone(),
                *ep,
                0,
                nact.tile_weak().clone(),
            );
            let sel = dst_sel
                .checked_add(1)
                .and_then(|s| s.checked_add(CapSel::try_from(i).unwrap()))
                .ok_or_else(|| kerrno(Code::LastCapOverflow))?;
            let scap = Capability::new(sel as CapSel, ep);
            parent_caps.insert_as_child(scap, dst_sel)?;
        }
    }
    drop(parent_caps);

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, syscalls::CreateActivityReply {
        id: nact.id(),
        eps_start: eps,
    });
    send_reply(&act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn create_sem(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateSem = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "create_sem(dst={}, value={})", r.dst, r.value);

    let sem = SemObject::new(r.value);
    let cap = Capability::new(r.dst, sem);
    act.obj_caps().borrow_mut().insert(cap)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn create_map_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::CreateMap = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "create_map(dst={}, act={}, mgate={}, first={}, pages={}, perms={:?})",
        r.dst,
        r.act,
        r.mgate,
        r.first,
        r.pages,
        r.perms
    );

    let dst_act: TempRc<Activity> = act.get_kobj(r.act)?;
    if !platform::tile_desc(dst_act.tile_id()).has_virtmem() {
        return Err(kerrno(Code::InvArgs).context("Tile has no virtual-memory support"));
    }

    let mgate: TempRc<MGateObject> = act.get_kobj(r.mgate)?;
    if (mgate.addr().raw() & cfg::PAGE_MASK as GlobOff) != 0
        || (mgate.size() & cfg::PAGE_MASK as GlobOff) != 0
    {
        return Err(kerrno(Code::InvArgs).context(format!(
            "Memory capability is not page aligned (addr={}, size={:#x})",
            mgate.addr(),
            mgate.size()
        )));
    }
    if (r.perms.bits() & !mgate.perms().bits()) != 0 {
        return Err(kerrno(Code::InvArgs).context("Invalid permissions"));
    }

    let total_pages = (mgate.size() >> cfg::PAGE_BITS) as CapSel;
    if r.first.checked_add(r.pages).is_none()
        || r.pages == 0
        || r.first >= total_pages
        || r.first + r.pages > total_pages
    {
        return Err(kerrno(Code::InvArgs).context("Region of memory cap is invalid"));
    }

    let virt = VirtAddr::new((r.dst as VirtAddrRaw) << (cfg::PAGE_BITS) as VirtAddrRaw);
    let base = mgate.addr().raw();
    let phys = GlobAddr::new(base + (cfg::PAGE_SIZE * r.first as usize) as u64);
    drop(mgate);

    // retrieve/create map object
    let (map_obj, _map_obj_clone, exists) = {
        let map_caps = dst_act.map_caps().borrow();
        let map_cap = map_caps.try_get(r.dst);
        match map_cap {
            Some(c) => {
                // TODO check for kernel-created caps
                // TODO we have to update MemGates that are childs of this cap
                if c.len() != r.pages {
                    return Err(
                        kerrno(Code::InvArgs).context("Map cap exists with different page count")
                    );
                }

                (c.get::<TempRc<MapObject>>()?, None, true)
            },
            None => {
                // TODO TOCTOU as multiple maps can race creating two mappings
                // for the same range simultaniously.
                let range = CapRngDesc::new(CapType::Mapping, r.dst, r.pages).map_err(|e| {
                    kerror(e).context(format!("Invalid cap range {}:{}", r.dst, r.pages))
                })?;
                if !map_caps.range_unused(&range) {
                    return Err(kerrno(Code::InvArgs)
                        .context(format!("Capability range {} already in use", range)));
                }

                // ensure that we keep a copy to not lose it during the async call
                let map_obj = MapObject::new(phys, PageFlags::from(r.perms));
                (TempRc::new(map_obj.clone()), Some(map_obj), false)
            },
        }
    };

    let dst_act_weak = dst_act.clone().downgrade_asyn();

    // drop before async call
    let (act_id, act_tile) = (dst_act.id(), dst_act.tile_id());
    let act_weak = act.downgrade_asyn();
    let map_obj_weak = map_obj.clone().downgrade_asyn();
    drop(dst_act);

    // create/update the PTEs
    if let Err(e) = MapObject::map_async(
        map_obj,
        act_id,
        act_tile,
        virt,
        phys,
        r.pages as usize,
        PageFlags::from(r.perms),
    ) {
        return Err(e.context("Unable to map memory"));
    }

    // create map cap, if not yet existing
    let act = if !exists {
        // if we cannot upgrade the destination activity or map object, we just created mapping was
        // already unmapped again.
        let dst_act = try_upgrade_kobj(dst_act_weak, r.act)?;
        let map_obj = try_upgrade_kobj(map_obj_weak, INVALID_SEL)?;
        drop(_map_obj_clone);

        if let Some(act) = act_weak.upgrade() {
            let map_obj = TempRc::into_strong(map_obj).unwrap();
            let cap = Capability::new_range(SelRange::new_range(r.dst, r.pages), map_obj);
            dst_act.map_caps().borrow_mut().insert_as_child_from(
                cap,
                act.obj_caps().borrow_mut(),
                r.mgate,
            )?;
            Some(act)
        }
        else {
            // if we fail to upgrade the syscall-performing activity, we cannot insert the mapping
            // and thus have to unmap it at TileMux again
            MapObject::unmap_async(act_id, act_tile, virt, r.pages as usize);
            None
        }
    }
    else {
        act_weak.upgrade()
    };

    if let Some(act) = act {
        reply_success(&act);
    }
    Ok(())
}
