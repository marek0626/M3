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
use base::errors::Code;
use base::kif::{self, syscalls, TileType};
use base::mem::MsgBuf;
use base::quota::Quota;
use base::{format, tcu};

use thread::{Downgradable, TempRc, Upgradable};

use crate::cap::{Capability, MGateObject, TileObject};
use crate::kerrno;
use crate::syscalls::{get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, TileMux, INVAL_ID};
use crate::{ktcu, platform};

#[inline(never)]
pub fn tile_quota_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileQuota = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "tile_quota(tile={})", r.tile);

    let tile: TempRc<TileObject> = act.get_kobj(r.tile)?;

    let tile_weak = tile.clone().downgrade_asyn();
    let tile_id = tile.tile();
    let time_quota_id = tile.time_quota_id();
    let pt_quota_id = tile.pt_quota_id();
    let act_weak = act.downgrade_asyn();
    drop(tile);

    let (time, pts) = if platform::tile_desc(tile_id).supports_tilemux() {
        if tilemng::tilemux(tile_id).is_initialized() {
            TileMux::get_quota_async(tilemng::tilemux(tile_id), time_quota_id, pt_quota_id)
                .map_err(|e| {
                    e.context(format!(
                        "Unable to get quota for time={}, pts={}",
                        time_quota_id, pt_quota_id,
                    ))
                })?
        }
        else {
            // fall back to defaults if TileMux isn't available
            (Quota::default(), Quota::default())
        }
    }
    else {
        (Quota::default(), Quota::default())
    };

    let tile = try_upgrade_kobj(tile_weak, r.tile)?;

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::TileQuotaReply {
        eps_id: tile.ep_quota().id(),
        eps_total: tile.ep_quota().total(),
        eps_left: tile.ep_quota().left(),
        exregs_id: tile.exregs_quota().id(),
        exregs_total: tile.exregs_quota().total(),
        exregs_left: tile.exregs_quota().left(),
        time_id: time.id(),
        time_total: time.total(),
        time_left: time.remaining(),
        pts_id: pts.id(),
        pts_total: pts.total(),
        pts_left: pts.remaining(),
    });

    if let Some(act) = act_weak.upgrade() {
        send_reply(&act, &kreply);
    }

    Ok(())
}

#[inline(never)]
pub fn tile_set_quota_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileSetQuota = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "tile_set_quota(tile={}, time={}, pts={})",
        r.tile,
        r.time,
        r.pts
    );

    let tile: TempRc<TileObject> = act.get_kobj(r.tile)?;

    if platform::tile_desc(tile.tile()).tile_type() == TileType::Mem {
        return Err(kerrno(Code::InvArgs).context("Cannot set quota for memory tiles"));
    }
    if tile.derived() {
        return Err(
            kerrno(Code::NoPerm).context("Cannot set tile quota with derived tile capability")
        );
    }
    if tile.activities() > 1 {
        return Err(kerrno(Code::InvArgs)
            .context("Cannot set tile quota with more than one Activity on the tile"));
    }

    let tilemux = tilemng::tilemux(tile.tile());
    let quota_id = tile.time_quota_id();
    let act_weak = act.downgrade_asyn();
    drop(tile);

    // the root tile object has always the same id for the time quota and the pts quota
    TileMux::set_quota_async(tilemux, quota_id, r.time, r.pts)?;

    if let Some(act) = act_weak.upgrade() {
        reply_success(&act);
    }
    Ok(())
}

#[inline(never)]
pub fn tile_set_pmp(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileSetPMP = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "tile_set_pmp(tile={}, mgate={}, ep={}, overwrite={})",
        r.tile,
        r.mgate,
        r.ep,
        r.overwrite
    );

    let act_caps = act.obj_caps().borrow();
    let tile: TempRc<TileObject> = act_caps.get_kobj(r.tile)?;
    if platform::tile_desc(tile.tile()).tile_type() == TileType::Mem {
        return Err(kerrno(Code::InvArgs).context("Cannot set PMP EPs for memory tiles"));
    }
    if tile.derived() {
        return Err(kerrno(Code::NoPerm).context("Cannot set PMP EPs for derived tile objects"));
    }
    if r.overwrite && tile.activities() > 0 {
        return Err(
            kerrno(Code::InvState).context("Cannot overwrite PMP EPs with existing activities")
        );
    }

    if r.ep < 1 || r.ep >= tcu::PMEM_PROT_EPS as tcu::EpId {
        return Err(kerrno(Code::InvArgs).context(format!(
            "Only EPs 1..{} can be used for tile_set_pmp",
            tcu::PMEM_PROT_EPS
        )));
    }

    let mut tilemux = tilemng::tilemux(tile.tile());

    let mgate: Option<TempRc<MGateObject>> = if r.mgate != kif::INVALID_SEL {
        Some(act_caps.get_kobj(r.mgate)?)
    }
    else {
        // invalidate EP if requested
        if let Err(e) = tilemux.invalidate_ep(INVAL_ID, r.ep, true, false) {
            return Err(e.context("Unable to invalidate PMP EP"));
        }

        None
    };

    tilemux.reconfigure_pmp_ep(r.ep, mgate, r.overwrite)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn tile_reset_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileReset = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "tile_reset(tile={}, mux_mem={}, ep_count={:?})",
        r.tile,
        r.mux_mem,
        r.ep_count
    );

    let act_caps = act.obj_caps().borrow();
    let tile: TempRc<TileObject> = act_caps.get_kobj(r.tile)?;
    if platform::tile_desc(tile.tile()).tile_type() == TileType::Mem {
        return Err(kerrno(Code::InvArgs).context("Cannot reset memory tiles"));
    }
    if tile.derived() {
        return Err(kerrno(Code::NoPerm).context("Cannot reset tiles for derived tile objects"));
    }

    let mux_mem = if r.mux_mem == kif::INVALID_SEL {
        None
    }
    else {
        // tiles that have internal EPs do not support external EPs and tiles without internal EPs need
        // external EPs.
        if platform::tile_desc(tile.tile()).has_internal_eps() != r.ep_count.is_none() {
            return Err(kerrno(Code::InvArgs).context("Tile-internal EPs vs. external EP range"));
        }

        Some(act_caps.get_kobj::<TempRc<MGateObject>>(r.mux_mem)?.clone())
    };
    drop(act_caps);

    let act_weak = act.downgrade_asyn();

    let tile_id = tile.tile();
    TileMux::reset_async(tile_id, Some(tile), mux_mem, r.ep_count, false)?;

    if let Some(act) = act_weak.upgrade() {
        reply_success(&act);
    }
    Ok(())
}

#[inline(never)]
pub fn tile_info(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileInfo = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "tile_info(tile={})", r.tile);

    let act_caps = act.obj_caps().borrow();
    let tile: TempRc<TileObject> = act_caps.get_kobj(r.tile)?;

    let tilemux = tilemng::tilemux(tile.tile());
    let ty = tilemux.mux_type();

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::TileInfoReply {
        ty,
        id: tile.tile(),
        desc: platform::tile_desc(tile.tile()),
        ep_count: ktcu::get_ep_count(tile.tile())?,
    });
    send_reply(act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn tile_mem(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileMem = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "tile_mem(dst={}, tile={})", r.dst, r.tile);

    let mut act_caps = act.obj_caps().borrow_mut();
    let tile: TempRc<TileObject> = act_caps.get_kobj(r.tile)?;
    if platform::tile_desc(tile.tile()).tile_type() == TileType::Mem {
        return Err(kerrno(Code::InvArgs).context("Cannot create memory cap for memory tiles"));
    }
    if tile.derived() {
        return Err(
            kerrno(Code::NoPerm).context("Cannot create memory cap for derived tile objects")
        );
    }
    if !platform::tile_desc(tile.tile()).has_memory() {
        return Err(kerrno(Code::InvArgs).context("Tile has no internal memory"));
    }

    let mem = tile.memory();
    let mgate = MGateObject::new(mem, kif::Perm::RWX, true);
    let cap = Capability::new(r.dst, mgate);
    act_caps.insert_as_child(cap, r.tile)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn tile_lock(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::TileLock = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "tile_lock(tile={})", r.tile);

    let tile: TempRc<TileObject> = act.obj_caps().borrow().get_kobj(r.tile)?;
    if platform::tile_desc(tile.tile()).tile_type() != TileType::Comp {
        return Err(kerrno(Code::InvArgs).context("Can only lock compute tiles"));
    }
    if tile.derived() {
        return Err(kerrno(Code::NoPerm).context("Cannot lock derived tile objects"));
    }

    let tilemux = tilemng::tilemux(tile.tile());
    if tilemux.is_locked() {
        return Err(kerrno(Code::Exists).context("Tile already locked"));
    }

    let eps_region = tilemux.eps_region();
    drop(tilemux);

    let tile_id = tile.tile();
    if let Some(eps_region) = eps_region {
        let epmtile = tilemng::ep_mem_tile();
        let mut exregs = tilemng::exregs(epmtile.tile());
        exregs.add(eps_region, epmtile, &tile)?;
    }

    let mut tilemux = tilemng::tilemux(tile.tile());
    tilemux.lock();
    ktcu::lock_tile(tile_id).unwrap();

    reply_success(act);
    Ok(())
}
