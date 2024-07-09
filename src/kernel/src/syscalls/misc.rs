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
use base::errors::Error;
use base::errors::{Code, VerboseError};
use base::kif::{self, syscalls};
use base::mem::{GlobOff, MsgBuf, PhysAddr, PhysAddrRaw};
use base::tcu;

use thread::AsyncRc;

use crate::cap::{Capability, EPCategory, EPObject, GateObject, KObject, SemObject};
use crate::ktcu;
use crate::platform;
use crate::syscalls::{check_unused, get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, TileMux};

#[inline(never)]
pub fn alloc_ep_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::AllocEP = get_request(msg)?;
    sysc_log!(
        act,
        "alloc_ep(dst={}, act={}, epid={}, replies={})",
        r.dst,
        r.act,
        r.epid,
        r.replies
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;
    if r.replies > cfg::MAX_RB_SIZE {
        sysc_err!(Code::InvArgs, "Invalid reply count ({})", r.replies);
    }

    let ep_count = 1 + r.replies as usize;
    let dst_act = get_kobj!(act, r.act, Activity);
    if !dst_act.tile().has_quota(ep_count) {
        sysc_err!(
            Code::NoSpace,
            "Tile cap has insufficient EPs (have {}, need {})",
            dst_act.tile().ep_quota().left(),
            ep_count
        );
    }

    let mut tilemux = tilemng::tilemux(dst_act.tile_id());

    let (act, dst_act, epid) = if tilemux.mux_type() == kif::syscalls::MuxType::Accel {
        let act_weak = act.downgrade();
        let dst_act_id = dst_act.id();
        let dst_act_weak = dst_act.downgrade();

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
            Err(e) => sysc_err!(e.code(), "No free EP range for {} EPs", ep_count),
        }
    }
    else {
        let avail_eps = tilemux.ep_count().unwrap();
        if r.epid as usize > avail_eps || r.epid as usize + ep_count > avail_eps {
            sysc_err!(
                Code::InvArgs,
                "Invalid endpoint id ({}:{})",
                r.epid,
                ep_count
            );
        }
        if !tilemux.eps_free(r.epid, ep_count) {
            sysc_err!(
                Code::InvArgs,
                "Endpoints {}..{} not free",
                r.epid,
                r.epid as usize + ep_count - 1
            );
        }
        (act, dst_act, r.epid)
    };

    let ep = EPObject::new(
        EPCategory::Custom,
        dst_act.clone().downgrade(),
        epid,
        r.replies,
        dst_act.tile_weak().clone(),
    );
    let cap = Capability::new(r.dst, create_kobj!(ep, EP));
    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.act));

    dst_act.tile().alloc(ep_count);
    tilemux.alloc_eps(epid, ep_count);

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::AllocEPReply {
        ep: epid
    });
    send_reply(msg, &kreply);

    Ok(())
}

#[inline(never)]
pub fn mgate_region(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::MGateRegion = get_request(msg)?;
    sysc_log!(act, "mgate_addr(mgate={})", r.mgate);

    let act_caps = act.obj_caps().borrow();
    let mgate = get_kobj_ref!(act_caps, r.mgate, MGate);

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::MGateRegionReply {
        global: mgate.addr(),
        size: mgate.size(),
    });
    send_reply(msg, &kreply);

    Ok(())
}

#[inline(never)]
pub fn rgate_buffer(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::RGateBuffer = get_request(msg)?;
    sysc_log!(act, "rgate_buffer(rgate={})", r.rgate);

    let act_caps = act.obj_caps().borrow();
    let rgate = get_kobj_ref!(act_caps, r.rgate, RGate);

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::RGateBufferReply {
        order: rgate.order(),
        msg_order: rgate.msg_order(),
    });
    send_reply(msg, &kreply);

    Ok(())
}

#[inline(never)]
pub fn kmem_quota(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::KMemQuota = get_request(msg)?;
    sysc_log!(act, "kmem_quota(kmem={})", r.kmem);

    let act_caps = act.obj_caps().borrow();
    let kmem = get_kobj_ref!(act_caps, r.kmem, KMem);

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, kif::syscalls::KMemQuotaReply {
        id: kmem.id(),
        total: kmem.quota(),
        left: kmem.left(),
    });
    send_reply(msg, &kreply);

    Ok(())
}

#[inline(never)]
pub fn get_sess(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::GetSess = get_request(msg)?;
    sysc_log!(
        act,
        "get_sess(dst={}, srv={}, act={}, sid={})",
        r.dst,
        r.srv,
        r.act,
        r.sid
    );

    let actcap = get_kobj!(act, r.act, Activity);
    check_unused(&actcap.obj_caps().borrow(), r.dst)?;
    if act.ptr_eq(&actcap) {
        sysc_err!(Code::InvArgs, "Cannot get session for own Activity");
    }

    // get service cap
    let mut act_caps = act.obj_caps().borrow_mut();
    let srvcap = act_caps
        .get_mut(r.srv)
        .ok_or_else(|| VerboseError::new(Code::InvArgs, "Invalid capability".to_string()))?;
    let creator = cap_to_kobj!(srvcap, Serv).creator();

    // find root service cap
    let srv_root = srvcap.get_root();

    // walk through the childs to find the session with given id (only root cap can create sessions)
    // safety: we don't keep the reference across an async call here
    let mut csess = srv_root
        .find_child(|c| matches!(unsafe { c.get() }, KObject::Sess(s) if s.ident() == r.sid));
    if let Some(KObject::Sess(s)) = csess.as_mut().map(|c| unsafe { c.get().clone() }) {
        if s.creator() != creator {
            sysc_err!(Code::NoPerm, "Cannot get access to foreign session");
        }

        try_kmem_quota!(actcap
            .obj_caps()
            .borrow_mut()
            .obtain(r.dst, csess.unwrap(), true));
    }
    else {
        sysc_err!(Code::InvArgs, "Unknown session id {}", r.sid);
    }

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn activate_mgate(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::ActivateMGate = get_request(msg)?;
    sysc_log!(act, "activate_mgate(ep={}, gate={})", r.ep, r.gate,);

    let ep = get_kobj!(act, r.ep, EP);
    if ep.replies() != 0 {
        sysc_err!(Code::InvArgs, "Only rgates use EP caps with reply slots");
    }

    if let Err(e) = ep.deconfigure(false) {
        sysc_err!(
            e.code(),
            "Invalidation of EP {}:{} failed",
            ep.tile_id(),
            ep.ep()
        );
    }

    let mg = get_kobj!(act, r.gate, MGate);

    if mg.gate_ep().get_ep().is_some() {
        sysc_err!(Code::Exists, "MemGate is already activated");
    }

    let tile_id = mg.tile_id();
    if let Err(e) = tilemng::tilemux(ep.tile_id()).config_mem_ep(
        ep.ep(),
        ep.activity().unwrap().id(),
        &mg,
        tile_id,
    ) {
        sysc_err!(e.code(), "Unable to configure mem EP");
    }

    mg.set_ep(&ep, GateObject::Mem(mg.clone().downgrade()));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn activate_rgate(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::ActivateRGate = get_request(msg)?;
    sysc_log!(
        act,
        "activate_rgate(ep={}, gate={}, rbuf_mem={}, rbuf_off={:#x})",
        r.ep,
        r.gate,
        r.rbuf_mem,
        r.rbuf_off,
    );

    let ep = get_kobj!(act, r.ep, EP);

    // activity that is currently active on the endpoint
    let ep_act = ep.activity().unwrap();

    let epid = ep.ep();
    let dst_tile = ep.tile_id();

    if let Err(e) = ep.deconfigure(false) {
        sysc_err!(e.code(), "Invalidation of EP {}:{} failed", dst_tile, epid);
    }

    let rg = get_kobj!(act, r.gate, RGate);
    if rg.activated() {
        sysc_err!(Code::Exists, "RecvGate is already activated");
    }

    // determine receive buffer address
    let dst_desc = platform::tile_desc(dst_tile);
    let rbuf_addr = if dst_desc.has_virtmem() && epid == ep_act.eps_start() + tcu::PG_REP_OFF {
        // special case for activating the pager reply rgate: there is no way to get a
        // memory capability to the standard receive buffer. thus, we just determine the
        // physical address here and remove the choice for the user.
        ep_act.rbuf_addr()
            + cfg::SYSC_RBUF_SIZE as PhysAddrRaw
            + cfg::UPCALL_RBUF_SIZE as PhysAddrRaw
            + cfg::DEF_RBUF_SIZE as PhysAddrRaw
    }
    else if dst_desc.has_virtmem() {
        let rbuf = get_kobj!(act, r.rbuf_mem, MGate);
        if r.rbuf_off >= rbuf.size() || r.rbuf_off + rg.size() as GlobOff > rbuf.size() {
            sysc_err!(Code::InvArgs, "Invalid receive buffer memory");
        }
        if platform::tile_desc(rbuf.tile_id()).tile_type() != kif::TileType::Mem {
            sysc_err!(Code::InvArgs, "rbuffer not in physical memory");
        }
        let rbuf_phys = ktcu::glob_to_phys_remote(dst_tile, rbuf.addr(), kif::PageFlags::RW)
            .map_err(|e| {
                VerboseError::new(
                    e.code(),
                    base::format!("Receive buffer at {} not accessible via PMP", rbuf.addr()),
                )
            })?;
        rbuf_phys + r.rbuf_off as PhysAddrRaw
    }
    else {
        if r.rbuf_mem != kif::INVALID_SEL {
            sysc_err!(Code::InvArgs, "rbuffer mem cap given for SPM tile");
        }
        PhysAddr::new_raw(dst_desc, r.rbuf_off as PhysAddrRaw)
    };

    let replies = if ep.replies() > 0 {
        let slots = 1 << (rg.order() - rg.msg_order());
        if ep.replies() != slots {
            sysc_err!(
                Code::InvArgs,
                "EP cap has {} reply slots, need {}",
                ep.replies(),
                slots
            );
        }
        Some(epid + 1)
    }
    else {
        None
    };

    rg.activate(ep_act.tile_id(), epid, rbuf_addr);

    if let Err(e) = tilemng::tilemux(dst_tile).config_rcv_ep(epid, ep_act.id(), replies, &rg) {
        rg.deactivate();
        sysc_err!(e.code(), "Unable to configure recv EP");
    }

    rg.set_ep(&ep, GateObject::Recv(rg.clone().downgrade()));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn activate_sgate_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::ActivateSGate = get_request(msg)?;
    sysc_log!(act, "activate_sgate(ep={}, gate={})", r.ep, r.gate,);

    let ep = get_kobj!(act, r.ep, EP);
    if ep.replies() != 0 {
        sysc_err!(Code::InvArgs, "Only rgates use EP caps with reply slots");
    }

    let epid = ep.ep();
    let dst_tile = ep.tile_id();

    if let Err(e) = ep.deconfigure(false) {
        sysc_err!(e.code(), "Invalidation of EP {}:{} failed", dst_tile, epid);
    }

    let sg = get_kobj!(act, r.gate, SGate);
    if sg.gate_ep().get_ep().is_some() {
        sysc_err!(Code::Exists, "SendGate is already activated");
    }

    let rgate = sg.rgate().ok_or_else(|| Error::new(Code::ObjectGone))?;

    let (ep, sg) = if !rgate.activated() {
        sysc_log!(act, "activate: waiting for rgate {:?}", *rgate);
        let ep_weak = ep.downgrade();
        let sg_weak = sg.downgrade();
        let event = rgate.get_event();
        let rg_weak = rgate.downgrade();
        let act_weak = act.downgrade();
        thread::wait_for(event);

        let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
        let rgate = try_upgrade_kobj(rg_weak, kif::INVALID_SEL)?;
        sysc_log!(act, "activate: rgate {:?} is activated", *rgate);
        let ep = try_upgrade_kobj(ep_weak, r.ep)?;
        let sg = try_upgrade_kobj(sg_weak, r.gate)?;
        (ep, sg)
    }
    else {
        (ep, sg)
    };

    if let Err(e) = tilemng::tilemux(dst_tile).config_snd_ep(epid, ep.activity().unwrap().id(), &sg)
    {
        sysc_err!(e.code(), "Unable to configure send EP");
    }

    sg.set_ep(&ep, GateObject::Send(sg.clone().downgrade()));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn invalidate(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::Invalidate = get_request(msg)?;
    sysc_log!(act, "invalidate(ep={})", r.ep);

    let ep = get_kobj!(act, r.ep, EP);

    if let Err(e) = tilemng::tilemux(ep.tile_id()).invalidate_ep(
        ep.activity().unwrap().id(),
        ep.ep(),
        !ep.is_rgate(),
        true,
    ) {
        sysc_err!(
            e.code(),
            "Invalidation of EP {}:{} failed",
            ep.tile_id(),
            ep.ep()
        );
    }

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn sem_ctrl_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::SemCtrl = get_request(msg)?;
    sysc_log!(act, "sem_ctrl(sem={}, op={:?})", r.sem, r.op);

    let sem = get_kobj!(act, r.sem, Sem);

    match r.op {
        kif::syscalls::SemOp::Up => {
            sem.up();
        },

        kif::syscalls::SemOp::Down => {
            let act_weak = act.downgrade();

            let res = SemObject::down_async(sem);

            let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
            sysc_log!(act, "sem_ctrl-cont(res={:?})", res);
            if let Err(e) = res {
                sysc_err!(e.code(), "Semaphore operation failed");
            }
        },
    }

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn activity_ctrl_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::ActivityCtrl = get_request(msg)?;
    sysc_log!(
        act,
        "activity_ctrl(act={}, op={:?}, arg={:#x})",
        r.act,
        r.op,
        r.arg
    );

    let actcap = get_kobj!(act, r.act, Activity);

    match r.op {
        kif::syscalls::ActivityOp::Start => {
            if act.ptr_eq(&actcap) {
                sysc_err!(Code::InvArgs, "Activity can't start itself");
            }
            drop(act);

            if let Err(e) = Activity::start_app_async(actcap) {
                sysc_err!(e.code(), "Unable to start Activity");
            }
        },

        kif::syscalls::ActivityOp::Stop => {
            let is_self = r.act == kif::SEL_ACT;
            let act_id = act.id();
            drop(act);

            Activity::stop_app_async(actcap, Code::from(r.arg as u32), is_self, act_id);
            if is_self {
                msg.ack();
                return Ok(());
            }
        },
    };

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn activity_wait_async(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::ActivityWait = get_request(msg)?;
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

    let act_weak = act.clone().downgrade();

    // In any case, check whether a activity already exited. If event == 0, wait until that happened.
    // For event != 0, remember that we want to get notified and send an upcall on a activity's exit.
    if let Some((sel, code)) = Activity::wait_exit_async(act, r.event, &r.acts[0..r.act_count]) {
        let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
        sysc_log!(act, "act_wait-cont(act={}, exitcode={:?})", sel, code);

        reply_msg.act_sel = sel;
        reply_msg.exitcode = code;
    }

    let mut reply = MsgBuf::borrow_def();
    build_vmsg!(reply, Code::Success, reply_msg);
    send_reply(msg, &reply);

    Ok(())
}

pub fn reset_stats(
    act: AsyncRc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    sysc_log!(act, "reset_stats()",);

    for tile in platform::user_tiles() {
        // ignore failures in case the TileMux is not available
        tilemng::tilemux(tile).reset_stats().ok();
    }

    reply_success(msg);
    Ok(())
}

pub fn noop(act: AsyncRc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    sysc_log!(act, "noop()",);

    reply_success(msg);
    Ok(())
}
