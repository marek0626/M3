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

use base::cfg;
use base::errors::Code;
use base::kif::syscalls::MuxType;
use base::kif::{self, syscalls};
use base::mem::{GlobOff, MsgBuf, PhysAddr, PhysAddrRaw};
use base::tcu;
use base::{build_vmsg, format};

use thread::{Downgradable, NonWeak, TempRc, Upgradable};

use crate::cap::{
    Capability, EPCategory, EPObject, GateObject, InvalidateType, KMemObject, MGateObject,
    RGateObject, SGateObject, SemObject, ServObject, SessObject, TileObject,
};
use crate::ktcu;
use crate::platform;
use crate::syscalls::{get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, ExRegs, TileMux};
use crate::{kerrno, kerror};

#[inline(never)]
pub fn alloc_ep_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::AllocEP = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "alloc_ep(dst={}, act={}, epid={}, replies={}, dyn={})",
        r.dst,
        r.act,
        r.epid,
        r.replies,
        r.dynamic
    );

    if r.replies > cfg::MAX_RB_SIZE {
        return Err(kerrno(Code::InvArgs).context(format!("Invalid reply count ({})", r.replies)));
    }

    let ep_count = 1 + r.replies as usize;
    let dst_act: TempRc<Activity> = act.get_kobj(r.act)?;
    if !dst_act.tile().has_ep_quota(ep_count) {
        return Err(kerrno(Code::NoSpace).context(format!(
            "Tile cap has insufficient EPs (have {}, need {})",
            dst_act.tile().ep_quota().left(),
            ep_count
        )));
    }

    let mut tilemux = tilemng::tilemux(dst_act.tile_id());

    let (act, dst_act, epid) = if tilemux.mux_type() == kif::syscalls::MuxType::Accel {
        let act_weak = act.downgrade_asyn();
        let dst_act_id = dst_act.id();
        let dst_act_weak = dst_act.downgrade_asyn();

        let epid = TileMux::request_ep_async(tilemux, dst_act_id, r.epid, r.replies)?;

        // if dst_act is gone, everything has already been cleaned up at TileMux
        let dst_act = try_upgrade_kobj(dst_act_weak, r.act)?;
        // in theory we would need to give them back if act is gone, but there is currently no way
        // to free them anyway, so that there is also nothing to do here.
        let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
        tilemux = tilemng::tilemux(dst_act.tile_id());
        (act, dst_act, epid)
    }
    else if r.epid == tcu::INVALID_EP {
        match tilemux.find_eps(ep_count) {
            Ok(epid) => (act, dst_act, epid),
            Err(e) => return Err(e.context(format!("No free EP range for {} EPs", ep_count))),
        }
    }
    else {
        let avail_eps = tilemux.ep_count().unwrap();
        if r.epid as usize > avail_eps || r.epid as usize + ep_count > avail_eps {
            return Err(kerrno(Code::InvArgs)
                .context(format!("Invalid endpoint id ({}:{})", r.epid, ep_count)));
        }
        if !tilemux.eps_free(r.epid, ep_count) {
            return Err(kerrno(Code::InvArgs).context(format!(
                "Endpoints {}..{} not free",
                r.epid,
                r.epid as usize + ep_count - 1
            )));
        }
        (act, dst_act, r.epid)
    };

    let ep = EPObject::new(
        EPCategory::Custom,
        dst_act.clone().downgrade_store(),
        epid,
        r.replies,
        dst_act.tile_weak().clone(),
    );
    // alloc EPs first and drop tilemux to ensure that the insert_as_child below can fail
    // if it fails, it will drop EPObject, which will acquire tilemux by itself and free the EPs
    dst_act.tile().alloc_eps(ep_count);
    tilemux.alloc_eps(epid, ep_count);
    drop(tilemux);

    let cap = Capability::new(r.dst, ep);
    act.obj_caps().borrow_mut().insert_as_child(cap, r.act)?;

    if r.dynamic {
        // make the EP invalid, but owned by the activity, so that it can make it dynamic if desired.
        // if the tile is not locked yet, we can also do it directly.
        ktcu::config_remote_ep(dst_act.tile_id(), epid, |regs, tgtep| {
            ktcu::config_invalid(regs, tgtep, dst_act.id(), true);
        })
        .unwrap();
    }

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::AllocEPReply {
        ep: epid
    });
    send_reply(&act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn mgate_region(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::MGateRegion = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "mgate_addr(mgate={})", r.mgate);

    let act_caps = act.obj_caps().borrow();
    let mgate: TempRc<MGateObject> = act_caps.get_kobj(r.mgate)?;

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::MGateRegionReply {
        global: mgate.addr(),
        size: mgate.size(),
    });
    send_reply(act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn mgate_mkexcl_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::MGateMkExcl = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "mgate_mkexcl(mgate={}, mem_tile={}, user_tile={}, locked={})",
        r.mgate,
        r.mem_tile,
        r.user_tile,
        r.locked,
    );

    let act_caps = act.obj_caps().borrow();
    let mgate: TempRc<MGateObject> = act_caps.get_kobj(r.mgate)?;
    let mem_tile: TempRc<TileObject> = act_caps.get_kobj(r.mem_tile)?;
    let user_tile: TempRc<TileObject> = act_caps.get_kobj(r.user_tile)?;
    drop(act_caps);

    if mem_tile.tile() != mgate.tile_id() {
        return Err(kerrno(Code::InvArgs).context("MGate needs to belong to the memory tile"));
    }

    let addr = mgate.offset();
    let size = mgate.size();
    if (size & 0x7) != 0 || !size.is_power_of_two() {
        return Err(kerrno(Code::InvArgs).context("Invalid size (need 8-byte aligned power of 2)"));
    }
    if (addr & 0x3) != 0 || ((addr >> 3) & ((size >> 3) - 1)) != 0 {
        return Err(kerrno(Code::InvArgs).context("Invalid address (need size-aligned)"));
    }

    let act_weak = act.downgrade_asyn();

    ExRegs::add_async(mgate, mem_tile, user_tile, r.locked)?;

    let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn rgate_buffer(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::RGateBuffer = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "rgate_buffer(rgate={})", r.rgate);

    let act_caps = act.obj_caps().borrow();
    let rgate: TempRc<RGateObject> = act_caps.get_kobj(r.rgate)?;

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::RGateBufferReply {
        order: rgate.order(),
        msg_order: rgate.msg_order(),
    });
    send_reply(act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn kmem_quota(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::KMemQuota = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "kmem_quota(kmem={})", r.kmem);

    let act_caps = act.obj_caps().borrow();
    let kmem: TempRc<KMemObject> = act_caps.get_kobj(r.kmem)?;

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::KMemQuotaReply {
        id: kmem.id(),
        total: kmem.quota(),
        left: kmem.left(),
    });
    send_reply(act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn get_sess(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::GetSess = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "get_sess(dst={}, srv={}, act={}, sid={})",
        r.dst,
        r.srv,
        r.act,
        r.sid
    );

    let actcap: TempRc<Activity> = act.get_kobj(r.act)?;
    if TempRc::ptr_eq(act, &actcap) {
        return Err(kerrno(Code::InvArgs).context("Cannot get session for own Activity"));
    }

    // get service cap
    let mut act_caps = act.obj_caps().borrow_mut();
    let srvcap = act_caps.get_mut(r.srv)?;
    let creator = srvcap.get::<TempRc<ServObject>>()?.creator();

    // find root service cap
    let srv_root = srvcap.get_root();

    // walk through the childs to find the session with given id (only root cap can create sessions)
    let csess = srv_root.find_child(|c| {
        if let Ok(s) = c.get::<TempRc<SessObject>>() {
            if s.ident() == r.sid {
                return true;
            }
        }
        false
    });
    if let Some(s) = csess
        .as_ref()
        .and_then(|c| c.get::<TempRc<SessObject>>().ok())
    {
        if s.creator() != creator {
            return Err(kerrno(Code::NoPerm).context("Cannot get access to foreign session"));
        }

        actcap
            .obj_caps()
            .borrow_mut()
            .obtain(r.dst, csess.unwrap())?;
    }
    else {
        return Err(kerrno(Code::InvArgs).context(format!("Unknown session id {}", r.sid)));
    }

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn activate_mgate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ActivateMGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "activate_mgate(ep={}, gate={})", r.ep, r.gate,);

    let ep: TempRc<EPObject> = act.get_kobj(r.ep)?;
    if ep.replies() != 0 {
        return Err(kerrno(Code::InvArgs).context("Only rgates use EP caps with reply slots"));
    }

    // invalidation not required as there is nothing to check and we'll overwrite it anyway
    if let Err(e) = ep.deconfigure(InvalidateType::None) {
        return Err(e.context(format!(
            "Invalidation of EP {}:{} failed",
            ep.tile_id(),
            ep.ep()
        )));
    }

    let mg: TempRc<MGateObject> = act.get_kobj(r.gate)?;

    if mg.gate_ep().get_ep().is_some() {
        return Err(kerrno(Code::Exists).context("MemGate is already activated"));
    }

    let tile_id = mg.tile_id();
    tilemng::tilemux(ep.tile_id()).config_mem_ep(
        ep.ep(),
        ep.activity().unwrap().id(),
        &mg,
        tile_id,
    )?;

    mg.set_ep(&ep, GateObject::Mem(mg.clone().downgrade_store()));

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn activate_rgate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ActivateRGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "activate_rgate(ep={}, gate={}, rbuf_mem={}, rbuf_off={:#x})",
        r.ep,
        r.gate,
        r.rbuf_mem,
        r.rbuf_off,
    );

    let ep: TempRc<EPObject> = act.get_kobj(r.ep)?;

    // activity that is currently active on the endpoint
    let ep_act = ep.activity().unwrap();

    let epid = ep.ep();
    let dst_tile = ep.tile_id();
    let mut tilemux = tilemng::tilemux(dst_tile);

    if let Err(e) = ep.deconfigure(InvalidateType::None) {
        return Err(e.context(format!("Invalidation of EP {}:{} failed", dst_tile, epid)));
    }

    let rg: TempRc<RGateObject> = act.get_kobj(r.gate)?;
    if rg.activated() {
        return Err(kerrno(Code::Exists).context("RecvGate is already activated"));
    }

    // determine receive buffer address
    let dst_desc = platform::tile_desc(dst_tile);
    let has_vm = dst_desc.has_virtmem() && tilemux.mux_type() != MuxType::Unimux;
    let rbuf_addr = if has_vm && epid == ep_act.eps_start() + tcu::PG_REP_OFF {
        // special case for activating the pager reply rgate: there is no way to get a
        // memory capability to the standard receive buffer. thus, we just determine the
        // physical address here and remove the choice for the user.
        ep_act.rbuf_addr()
            + cfg::SYSC_RBUF_SIZE as PhysAddrRaw
            + cfg::UPCALL_RBUF_SIZE as PhysAddrRaw
            + cfg::DEF_RBUF_SIZE as PhysAddrRaw
    }
    else if has_vm {
        let rbuf: TempRc<MGateObject> = act.get_kobj(r.rbuf_mem)?;
        if r.rbuf_off >= rbuf.size() || r.rbuf_off + rg.size() as GlobOff > rbuf.size() {
            return Err(kerrno(Code::InvArgs).context("Invalid receive buffer memory"));
        }
        if platform::tile_desc(rbuf.tile_id()).tile_type() != kif::TileType::Mem {
            return Err(kerrno(Code::InvArgs).context("rbuffer not in physical memory"));
        }
        let rbuf_phys = ktcu::glob_to_phys_remote(dst_tile, rbuf.addr(), kif::PageFlags::RW)
            .map_err(|e| {
                e.context(format!(
                    "Receive buffer at {} not accessible via PMP",
                    rbuf.addr(),
                ))
            })?;
        rbuf_phys + r.rbuf_off as PhysAddrRaw
    }
    else {
        if r.rbuf_mem != kif::INVALID_SEL {
            return Err(kerrno(Code::InvArgs).context("rbuffer mem cap given for SPM tile"));
        }
        PhysAddr::new_raw(dst_desc, r.rbuf_off as PhysAddrRaw)
    };

    let replies = if ep.replies() > 0 {
        let slots = 1 << (rg.order() - rg.msg_order());
        if ep.replies() != slots {
            return Err(kerrno(Code::InvArgs).context(format!(
                "EP cap has {} reply slots, need {}",
                ep.replies(),
                slots
            )));
        }
        Some(epid + 1)
    }
    else {
        None
    };

    rg.activate(ep_act.tile_id(), epid, rbuf_addr);

    if let Err(e) = tilemux.config_rcv_ep(epid, ep_act.id(), replies, &rg) {
        rg.deactivate();
        return Err(e.context("Unable to configure recv EP"));
    }

    rg.set_ep(&ep, GateObject::Recv(rg.clone().downgrade_store()));

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn activate_sgate_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ActivateSGate = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "activate_sgate(ep={}, gate={})", r.ep, r.gate,);

    let ep: TempRc<EPObject> = act.get_kobj(r.ep)?;
    if ep.replies() != 0 {
        return Err(kerrno(Code::InvArgs).context("Only rgates use EP caps with reply slots"));
    }

    let epid = ep.ep();
    let dst_tile = ep.tile_id();

    if let Err(e) = ep.deconfigure(InvalidateType::None) {
        return Err(e.context(format!("Invalidation of EP {}:{} failed", dst_tile, epid)));
    }

    let sg: TempRc<SGateObject> = act.get_kobj(r.gate)?;
    if sg.gate_ep().get_ep().is_some() {
        return Err(kerrno(Code::Exists).context("SendGate is already activated"));
    }

    let rgate = sg
        .rgate()
        .ok_or_else(|| kerrno(Code::ObjectGone).context("RGate was destroyed"))?;

    let (act, ep, sg) = if !rgate.activated() {
        sysc_log!(act, "activate: waiting for rgate {:?}", *rgate);
        let ep_weak = ep.downgrade_asyn();
        let sg_weak = sg.downgrade_asyn();
        let event = rgate.get_event();
        let rg_weak = rgate.downgrade_asyn();
        let act_weak = act.downgrade_asyn();
        thread::wait_many_async(event, &[&ep_weak, &sg_weak, &rg_weak, &act_weak]);

        let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
        let rgate = try_upgrade_kobj(rg_weak, kif::INVALID_SEL)?;
        sysc_log!(act, "activate: rgate {:?} is activated", *rgate);
        let ep = try_upgrade_kobj(ep_weak, r.ep)?;
        let sg = try_upgrade_kobj(sg_weak, r.gate)?;
        (act, ep, sg)
    }
    else {
        (act, ep, sg)
    };

    if let Err(e) = tilemng::tilemux(dst_tile).config_snd_ep(epid, ep.activity().unwrap().id(), &sg)
    {
        return Err(e.context("Unable to configure send EP"));
    }

    sg.set_ep(&ep, GateObject::Send(sg.clone().downgrade_store()));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn invalidate(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::Invalidate = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "invalidate(ep={})", r.ep);

    let ep: TempRc<EPObject> = act.get_kobj(r.ep)?;

    if let Err(e) = ep.deconfigure(InvalidateType::Default) {
        return Err(e.context(format!(
            "Invalidation of EP {}:{} failed",
            ep.tile_id(),
            ep.ep()
        )));
    }

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn sem_ctrl_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::SemCtrl = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "sem_ctrl(sem={}, op={:?})", r.sem, r.op);

    let sem: TempRc<SemObject> = act.get_kobj(r.sem)?;

    let act = match r.op {
        kif::syscalls::SemOp::Up => {
            sem.up();
            act
        },

        kif::syscalls::SemOp::Down => {
            let act_weak = act.downgrade_asyn();

            let res = SemObject::down_async(sem);

            let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
            sysc_log!(act, "sem_ctrl-cont(res={:?})", res);
            if let Err(e) = res {
                return Err(e.context("Semaphore operation failed"));
            }
            act
        },
    };

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn activity_ctrl_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ActivityCtrl = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "activity_ctrl(act={}, op={:?}, arg={:#x})",
        r.act,
        r.op,
        r.arg
    );

    let actcap: TempRc<Activity> = act.get_kobj(r.act)?;
    let act_weak = act.clone().downgrade_asyn();

    match r.op {
        kif::syscalls::ActivityOp::Start => {
            if TempRc::ptr_eq(&act, &actcap) {
                return Err(kerrno(Code::InvArgs).context("Activity can't start itself"));
            }
            drop(act);

            if let Err(e) = Activity::start_app_async(actcap) {
                return Err(e.context("Start activity"));
            }
        },

        kif::syscalls::ActivityOp::Stop => {
            let is_self = r.act == kif::SEL_ACT;
            let act_id = act.id();
            drop(act);

            let exitcode = Code::try_from(r.arg as u32).map_err(kerror)?;
            Activity::stop_app_async(actcap, exitcode, act_id);

            if is_self {
                // syscall message has already been invalidated
                return Ok(());
            }
        },
    }

    if let Some(act) = act_weak.upgrade() {
        reply_success(&act);
    }
    Ok(())
}

#[inline(never)]
pub fn activity_wait_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ActivityWait = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "activity_wait(activities={}, event={})",
        r.act_count,
        r.event
    );

    let mut reply_msg = kif::syscalls::ActivityWaitReply {
        act_sel: kif::INVALID_SEL,
        exitcode: Code::Success,
    };

    let act_weak = act.clone().downgrade_asyn();

    // In any case, check whether a activity already exited. If event == 0, wait until that happened.
    // For event != 0, remember that we want to get notified and send an upcall on a activity's exit.
    let res = Activity::wait_exit_async(act, r.event, &r.acts[0..r.act_count])?;

    let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;

    if let Some((sel, code)) = res {
        sysc_log!(act, "act_wait-cont(act={}, exitcode={:?})", sel, code);

        reply_msg.act_sel = sel;
        reply_msg.exitcode = code;
    }

    let mut reply = MsgBuf::borrow_def();
    build_vmsg!(reply, Code::Success, reply_msg);
    send_reply(&act, &reply);

    Ok(())
}

pub fn reset_stats(act: &TempRc<Activity>) -> anyhow::Result<()> {
    sysc_log!(act, "reset_stats()",);

    for tile in platform::user_tiles() {
        // ignore failures in case the TileMux is not available
        tilemng::tilemux(tile).reset_stats().ok();
    }

    reply_success(act);
    Ok(())
}

pub fn noop(act: &TempRc<Activity>) -> anyhow::Result<()> {
    sysc_log!(act, "noop()",);

    reply_success(act);
    Ok(())
}
