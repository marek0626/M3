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
use base::cfg;
use base::col::ToString;
use base::errors::{Code, VerboseError};
use base::kif::INVALID_SEL;
use base::kif::{syscalls, CapRngDesc, CapSel, CapType, PageFlags, Perm};
use base::mem::{GlobAddr, GlobOff, MsgBuf, VirtAddr, VirtAddrRaw};
use base::tcu;

use thread::AsyncRc;

use crate::cap::{
    Capability, EPCategory, EPObject, KMemObject, MGateObject, MapObject, RGateObject, SGateObject,
    SelRange, SemObject, ServObject, SessObject, TileObject,
};
use crate::com::Service;
use crate::mem;
use crate::platform;
use crate::syscalls::{check_unused, get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, ActivityFlags, ActivityMng};

#[inline(never)]
pub fn create_mgate(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateMGate = get_request(msg)?;
    sysc_log!(
        act,
        "create_mgate(dst={}, act={}, addr={}, size={:#x}, perms={:?})",
        r.dst,
        r.act,
        r.addr,
        r.size,
        r.perms,
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;
    if (r.addr.as_goff() & cfg::PAGE_MASK as GlobOff) != 0
        || (r.size & cfg::PAGE_MASK as GlobOff) != 0
    {
        sysc_err!(
            Code::InvArgs,
            "Virt address and size need to be page-aligned"
        );
    }

    let tgt_act: AsyncRc<Activity> = act.get_kobj(r.act)?;

    let sel = (r.addr.as_goff() / cfg::PAGE_SIZE as GlobOff) as CapSel;
    let glob = if platform::tile_desc(tgt_act.tile_id()).has_virtmem() {
        let pages = (r.size / cfg::PAGE_SIZE as GlobOff) as CapSel;
        if pages == 0 {
            sysc_err!(Code::InvArgs, "Region is empty");
        }

        let map_caps = tgt_act.map_caps().borrow();
        let map_cap = map_caps.get(sel)?;
        let map_obj: AsyncRc<MapObject> = map_cap.get()?;

        // TODO think about the flags in MapObject again
        let map_perms = Perm::from_bits_truncate(map_obj.flags().bits() as u32);
        if !(r.perms & !Perm::RWX).is_empty() || !(r.perms & !map_perms).is_empty() {
            sysc_err!(Code::NoPerm, "Invalid permissions");
        }

        let pages = (r.size / cfg::PAGE_SIZE as GlobOff) as CapSel;
        let off = sel - map_cap.sel();
        if off + pages > map_cap.len() {
            sysc_err!(Code::InvArgs, "Invalid length");
        }

        let phys =
            crate::ktcu::glob_to_phys_remote(tgt_act.tile_id(), map_obj.global(), map_obj.flags())?;
        GlobAddr::new_with(tgt_act.tile_id(), phys.as_goff())
    }
    else {
        if r.size == 0 {
            sysc_err!(Code::InvArgs, "Region is empty");
        }
        // use the same error code here as above where we fail with InvCap for non-existing mapping
        // capabilities (unmapped regions)
        if r.addr + r.size >= cfg::MEM_CAP_END {
            sysc_err!(Code::InvCap, "Region is out of bounds");
        }

        GlobAddr::new_with(tgt_act.tile_id(), r.addr.as_goff())
    };

    let mem = mem::Allocation::new(glob, r.size);
    let mgate = MGateObject::new(mem, r.perms, true);
    let cap = Capability::new(r.dst, mgate.into());

    if platform::tile_desc(tgt_act.tile_id()).has_virtmem() {
        let map_caps = tgt_act.map_caps().borrow_mut();
        try_kmem_quota!(act
            .obj_caps()
            .borrow_mut()
            .insert_as_child_from(cap, map_caps, sel));
    }
    else {
        try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.act));
    }

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_rgate(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateRGate = get_request(msg)?;
    sysc_log!(
        act,
        "create_rgate(dst={}, size={:#x}, msg_size={:#x})",
        r.dst,
        1u32.checked_shl(r.order).unwrap_or(0),
        1u32.checked_shl(r.msg_order).unwrap_or(0)
    );

    let mut act_caps = act.obj_caps().borrow_mut();

    check_unused(&act_caps, r.dst)?;
    if r.msg_order.checked_add(r.order).is_none()
        || r.msg_order > r.order
        || r.order - r.msg_order >= 32
        || (1 << (r.order - r.msg_order)) > cfg::MAX_RB_SIZE
    {
        sysc_err!(Code::InvArgs, "Invalid size");
    }

    let rgate = RGateObject::new(r.order, r.msg_order, false);
    try_kmem_quota!(act_caps.insert(Capability::new(r.dst, rgate.into())));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_sgate(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateSGate = get_request(msg)?;
    sysc_log!(
        act,
        "create_sgate(dst={}, rgate={}, label={:#x}, credits={})",
        r.dst,
        r.rgate,
        r.label,
        r.credits
    );

    let mut act_caps = act.obj_caps().borrow_mut();

    check_unused(&act_caps, r.dst)?;

    let cap = {
        let rgate: AsyncRc<RGateObject> = act_caps.get_kobj(r.rgate)?;
        let sgate = SGateObject::new(rgate.downgrade(), r.label, r.credits);
        Capability::new(r.dst, sgate.into())
    };

    try_kmem_quota!(act_caps.insert_as_child(cap, r.rgate));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_srv(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::CreateSrv<'_> = get_request(msg)?;
    sysc_log!(
        act,
        "create_srv(dst={}, rgate={}, creator={}, name={})",
        r.dst,
        r.rgate,
        r.creator,
        r.name
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;
    if r.name.is_empty() {
        sysc_err!(Code::InvArgs, "Invalid server name");
    }

    let mut act_caps = act.obj_caps().borrow_mut();

    let cap = {
        let rgate: AsyncRc<RGateObject> = act_caps.get_kobj(r.rgate)?;
        if !rgate.activated() {
            sysc_err!(Code::InvArgs, "RGate is not activated");
        }

        let serv = Service::new(act.clone(), r.name.to_string(), rgate);
        let serv_obj = ServObject::new(serv, true, r.creator);
        Capability::new(r.dst, serv_obj.into())
    };

    try_kmem_quota!(act_caps.insert(cap));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_sess(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateSess = get_request(msg)?;
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
    check_unused(&obj_caps, r.dst)?;

    let serv_cap = obj_caps.get(r.srv)?;
    // TODO maybe we should store that rather in the ServObject?
    if serv_cap.has_parent() {
        sysc_err!(Code::InvArgs, "Only the service owner can create sessions");
    }

    let serv: AsyncRc<ServObject> = serv_cap.get()?;
    let sess = SessObject::new(serv.downgrade(), r.creator, r.ident, r.auto_close);
    let cap = Capability::new(r.dst, sess.into());

    try_kmem_quota!(obj_caps.insert_as_child(cap, r.srv));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_activity_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateActivity<'_> = get_request(msg)?;
    sysc_log!(
        act,
        "create_activity(dst={}, name={}, tile={}, kmem={})",
        r.dst,
        r.name,
        r.tile,
        r.kmem
    );

    if !act
        .obj_caps()
        .borrow()
        .range_unused(&CapRngDesc::new(CapType::Object, r.dst, 3))
    {
        sysc_err!(
            Code::InvArgs,
            "Selectors {}..{} already in use",
            r.dst,
            r.dst + 2
        );
    }
    if r.name.is_empty() {
        sysc_err!(Code::InvArgs, "Invalid name");
    }

    let tile: AsyncRc<TileObject> = act.get_kobj(r.tile)?;
    if !tile.has_quota(tcu::STD_EPS_COUNT) {
        sysc_err!(
            Code::InvArgs,
            "Tile cap has insufficient EPs (have {}, need {})",
            tile.ep_quota().left(),
            tcu::STD_EPS_COUNT
        );
    }

    let kmem: AsyncRc<KMemObject> = act.get_kobj(r.kmem)?;
    // TODO kmem quota stuff

    // find contiguous space for standard EPs
    let tile_id = tile.tile();
    let tilemux = tilemng::tilemux(tile_id);
    let eps = match tilemux.find_eps(tcu::STD_EPS_COUNT) {
        Ok(eps) => eps,
        Err(e) => sysc_err!(e.code(), "No free range for standard EPs"),
    };
    if tilemux.has_activities() && !platform::tile_desc(tile.tile()).has_virtmem() {
        sysc_err!(Code::NotSup, "Virtual memory is required for tile sharing");
    }
    drop(tilemux);

    let act_weak = act.downgrade();

    // create activity
    let nact =
        match ActivityMng::create_activity_async(r.name, tile, eps, kmem, ActivityFlags::empty()) {
            Ok(nact) => nact,
            Err(e) => sysc_err!(e.code(), "Unable to create Activity"),
        };

    let act = try_upgrade_kobj(act_weak, INVALID_SEL)?;

    // give activity cap to the parent
    let cap = Capability::new(r.dst, nact.clone().into());
    try_kmem_quota!(act.obj_caps().borrow_mut().insert(cap));

    // create EP caps for the pager EPs
    if nact.tile_desc().has_virtmem() {
        let nact_weak = nact.clone().downgrade();
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
            let scap = Capability::new(r.dst + 1 + i as CapSel, ep.into());
            try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(scap, r.dst));
        }
    }

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, syscalls::CreateActivityReply {
        id: nact.id(),
        eps_start: eps,
    });
    send_reply(msg, &kreply);

    Ok(())
}

#[inline(never)]
pub fn create_sem(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::CreateSem = get_request(msg)?;
    sysc_log!(act, "create_sem(dst={}, value={})", r.dst, r.value);

    check_unused(&act.obj_caps().borrow(), r.dst)?;

    let sem = SemObject::new(r.value);
    let cap = Capability::new(r.dst, sem.into());
    try_kmem_quota!(act.obj_caps().borrow_mut().insert(cap));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn create_map_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::CreateMap = get_request(msg)?;
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

    let dst_act: AsyncRc<Activity> = act.get_kobj(r.act)?;
    if !platform::tile_desc(dst_act.tile_id()).has_virtmem() {
        sysc_err!(Code::InvArgs, "Tile has no virtual-memory support");
    }

    let mgate: AsyncRc<MGateObject> = act.get_kobj(r.mgate)?;
    if (mgate.addr().raw() & cfg::PAGE_MASK as GlobOff) != 0
        || (mgate.size() & cfg::PAGE_MASK as GlobOff) != 0
    {
        sysc_err!(
            Code::InvArgs,
            "Memory capability is not page aligned (addr={}, size={:#x})",
            mgate.addr(),
            mgate.size()
        );
    }
    if (r.perms.bits() & !mgate.perms().bits()) != 0 {
        sysc_err!(Code::InvArgs, "Invalid permissions");
    }

    let total_pages = (mgate.size() >> cfg::PAGE_BITS) as CapSel;
    if r.first.checked_add(r.pages).is_none()
        || r.pages == 0
        || r.first >= total_pages
        || r.first + r.pages > total_pages
    {
        sysc_err!(Code::InvArgs, "Region of memory cap is invalid");
    }

    let virt = VirtAddr::new((r.dst as VirtAddrRaw) << (cfg::PAGE_BITS) as VirtAddrRaw);
    let base = mgate.addr().raw();
    let phys = GlobAddr::new(base + (cfg::PAGE_SIZE * r.first as usize) as u64);
    drop(mgate);

    // retrieve/create map object
    let (map_obj, _map_obj_clone, exists) = {
        let map_caps = dst_act.map_caps().borrow();
        let map_cap = map_caps.get(r.dst);
        match map_cap {
            Ok(c) => {
                // TODO check for kernel-created caps
                // TODO we have to update MemGates that are childs of this cap
                if c.len() != r.pages {
                    sysc_err!(Code::InvArgs, "Map cap exists with different page count");
                }

                (c.get::<AsyncRc<MapObject>>()?, None, true)
            },
            Err(_) => {
                let range = CapRngDesc::new(CapType::Mapping, r.dst, r.pages);
                if !map_caps.range_unused(&range) {
                    sysc_err!(Code::InvArgs, "Capability range {} already in use", range);
                }

                // ensure that we keep a copy to not lose it during the async call
                let map_obj = MapObject::new(phys, PageFlags::from(r.perms));
                // safety: it's okay to keep the Rc here across the async call, because the object
                // was not inserted into the capability space yet and thus cannot be revoked
                let map_clone = unsafe { map_obj.inner().clone() };
                (map_obj, Some(map_clone), false)
            },
        }
    };

    let dst_act_weak = dst_act.clone().downgrade();

    // drop before async call
    let (act_id, act_tile) = (dst_act.id(), dst_act.tile_id());
    let act_weak = act.downgrade();
    let map_obj_weak = map_obj.clone().downgrade();
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
        sysc_err!(e.code(), "Unable to map memory");
    }

    // create map cap, if not yet existing
    if !exists {
        // if we cannot upgrade the destination activity or map object, we just created mapping was
        // already unmapped again.
        let dst_act = try_upgrade_kobj(dst_act_weak, r.act)?;
        let map_obj = try_upgrade_kobj(map_obj_weak, INVALID_SEL)?;

        if let Some(act) = act_weak.upgrade() {
            let cap = Capability::new_range(SelRange::new_range(r.dst, r.pages), map_obj.into());
            try_kmem_quota!(dst_act.map_caps().borrow_mut().insert_as_child_from(
                cap,
                act.obj_caps().borrow_mut(),
                r.mgate,
            ));
        }
        else {
            // if we fail to upgrade the syscall-performing activity, we cannot insert the mapping
            // and thus have to unmap it at TileMux again
            MapObject::unmap_async(act_id, act_tile, virt, r.pages as usize);
        }
    }

    reply_success(msg);
    Ok(())
}
