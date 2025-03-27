/*
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
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
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{GlobAddr, MsgBuf};
use base::serialize::M3Deserializer;
use base::tcu;
use base::time::TimeDuration;

use mux::sidecalls::*;

use crate::activities;
use crate::quota;
use mux::{helper, sendqueue};

fn activity_init(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ActInit = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::activity_init(act={}, time={}, pt={}, eps_start={})",
        r.act_id,
        r.time_quota,
        r.pt_quota,
        r.eps_start
    );

    activities::add(r.act_id, r.time_quota, r.pt_quota, r.eps_start).map(|_| (0, 0))
}

fn activity_ctrl(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ActivityCtrl = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::activity_ctrl(act={}, op={:?})",
        r.act_id,
        r.act_op,
    );

    match r.act_op {
        kif::tilemux::ActivityOp::Start => {
            let cur = activities::cur();
            assert!(cur.id() != r.act_id);
            let mut act = activities::get_mut(r.act_id).unwrap();
            // temporary switch to the activity to access the environment
            act.switch_to();
            act.start();
            act.unblock(activities::Event::Start);
            // now switch back
            cur.switch_to();
            Ok((0, 0))
        },

        _ => {
            // we cannot remove the current activity here; remove it via scheduling
            match activities::try_cur() {
                Some(cur) if cur.id() == r.act_id => {
                    crate::reg_scheduling(activities::ScheduleAction::Kill)
                },
                _ => activities::remove(r.act_id, Code::Success, false, true),
            }
            Ok((0, 0))
        },
    }
}

fn map(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::Map = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::map(act={}, virt={}, glob={}, pages={}, perm={:?})",
        r.act_id,
        r.virt,
        r.global,
        r.pages,
        r.perm
    );

    // ensure that we don't overmap critical areas
    let rbuf_space = crate::pex_env().tile_desc.rbuf_mux_space();
    if r.virt < rbuf_space.0 + rbuf_space.1
        || r.virt + r.pages * cfg::PAGE_SIZE > cfg::TILE_MEM_BASE
    {
        return Err(Error::new(Code::InvArgs));
    }

    if let Some(mut act) = activities::get_mut(r.act_id) {
        // if we unmap these pages, flush+invalidate the cache to ensure that we read this memory
        // fresh from DRAM the next time we use it.
        let perm = if (r.perm & kif::PageFlags::RWX).is_empty() {
            helper::flush_cache();
            r.perm
        }
        else {
            r.perm | kif::PageFlags::U
        };

        act.map(r.virt, r.global, r.pages, perm).map(|_| (0, 0))
    }
    else {
        Ok((0, 0))
    }
}

fn translate(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::Translate = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::translate(act={}, virt={}, perm={:?})",
        r.act_id,
        r.virt,
        r.perm
    );

    let (phys, flags) = activities::get_mut(r.act_id)
        .unwrap()
        .translate(r.virt, r.perm | kif::PageFlags::U);
    if (flags & r.perm) == kif::PageFlags::empty() {
        Err(Error::new(Code::NoPerm))
    }
    else {
        Ok((GlobAddr::new_from_phys(phys).unwrap().raw(), 0))
    }
}

fn rem_msgs(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::RemMsgs = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::rem_msgs(act={}, unread={})",
        r.act_id,
        r.unread_mask
    );

    // we know that this activity is not currently running, because we changed the current activity to ourself
    // in check() below.
    if let Some(mut act) = activities::get_mut(r.act_id) {
        act.rem_msgs(r.unread_mask.count_ones() as u16);
    }

    Ok((0, 0))
}

fn ep_inval(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::EpInval = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::ep_inval(act={}, ep={})",
        r.act_id,
        r.ep
    );

    // just unblock the activity in case it wants to do something on invalidated EPs
    if let Some(mut act) = activities::get_mut(r.act_id) {
        act.unblock(activities::Event::EpInvalid);
    }

    Ok((0, 0))
}

fn set_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::SetQuota = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::set_quota(id={}, time={:?}, pts={})",
        r.id,
        r.time,
        r.pts
    );

    quota::set(r.id, TimeDuration::from_nanos(r.time), r.pts).map(|_| (0, 0))
}

fn remove_quotas(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::RemoveQuotas = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::remove_quotas(time={:?}, pts={:?})",
        r.time,
        r.pts
    );

    quota::remove(r.time, r.pts).map(|_| (0, 0))
}

fn reset_stats(_msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::reset_stats()",);

    for id in 0..64 {
        if let Some(mut act) = activities::get_mut(id) {
            act.reset_stats();
        }
    }

    Ok((0, 0))
}

fn shutdown(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::shutdown()",);

    base::machine::write_coverage(0);

    let mut reply_buf = MsgBuf::borrow_def();
    base::build_vmsg!(reply_buf, Code::Success, kif::tilemux::Response {
        val1: 0,
        val2: 0
    });
    reply_msg(msg, &reply_buf);

    // call shutdown here directly after reply, so that we hopefully don't execute any code while
    // the kernel resets the tile. this is actually just a workaround for gem5, where we cannot
    // reset the core properly.
    extern "C" {
        fn _shutdown();
    }
    unsafe {
        _shutdown();
    }

    unreachable!();
}

fn info(_msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::info()",);
    Ok((kif::syscalls::MuxType::TileMux.into(), 0))
}

fn derive_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::DeriveQuota = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::derive_quota(ptime={}, ppts={}, time={:?}, pts={:?})",
        r.parent_time,
        r.parent_pts,
        r.time,
        r.pts
    );

    quota::derive(
        r.parent_time,
        r.parent_pts,
        r.time.map(TimeDuration::from_nanos),
        r.pts,
    )
}

fn get_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::GetQuota = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::get_quota(time={}, pts={})",
        r.time,
        r.pts
    );

    quota::get(r.time, r.pts).map(|(t_total, t_left, p_total, p_left)| {
        (
            (t_total << 32 | t_left),
            ((p_total as u64) << 32 | (p_left as u64)),
        )
    })
}

// Sidecalls are messages from the kernel to do many kinds of things at the basic resource level, such as changing mappings, managing an activity, etc.
fn handle_sidecall(msg: &'static tcu::Message) {
    let mut de = M3Deserializer::new(msg.as_words());

    let op: kif::tilemux::Sidecalls = de.pop().unwrap();
    let res = if let Some(handler) = find_handler(op) {
        handler(msg)
    }
    else {
        Err(Error::new(Code::NotSup))
    };

    let mut reply_buf = MsgBuf::borrow_def();
    match res {
        Ok(values) => {
            base::build_vmsg!(reply_buf, Code::Success, kif::tilemux::Response {
                val1: values.0,
                val2: values.1
            });
        },
        Err(e) => {
            log!(LogFlags::MuxSideCalls, "sidecall {:?} failed: {}", op, e);
            base::build_vmsg!(reply_buf, e.code(), kif::tilemux::Response {
                val1: 0,
                val2: 0
            });
        },
    }
    reply_msg(msg, &reply_buf);
}

#[inline(never)]
fn handle_sidecalls(mut our: activities::ActivityRef<'_>) {
    let _cmd_saved = helper::TCUGuard::new();

    loop {
        // change to our activity
        let old_act = tcu::TCU::xchg_activity(our.activity_reg()).unwrap();
        if let Some(mut old) = activities::try_cur() {
            old.set_activity_reg(old_act);
        }

        if let Some(msg_off) = tcu::TCU::fetch_msg(tcu::TMSIDE_REP) {
            let msg = tcu::TCU::offset_to_msg(side_rbuf_addr(), msg_off);
            handle_sidecall(msg);
        }

        // check if the kernel answered a request from us
        sendqueue::check_replies();

        // change back to old activity
        let new_act = activities::try_cur().map_or(old_act, |new| new.activity_reg());
        our.set_activity_reg(tcu::TCU::xchg_activity(new_act).unwrap());
        // if no events arrived in the meantime, we're done
        if !our.has_msgs() {
            break;
        }
    }
}

// Called at the end of every interrupt handling routine to check if we've received a kernel message of any kind.
#[inline(always)]
pub fn check() {
    let our = activities::our();
    if !our.has_msgs() {
        return;
    }

    handle_sidecalls(our);
}

pub fn basic_handlers_init() {
    register_sidecall_handler(kif::tilemux::Sidecalls::ActInit, activity_init).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::RemMsgs, rem_msgs).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::EPInval, ep_inval).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Shutdown, shutdown).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Info, info).ok();
}

pub fn tilemux_handlers_init() {
    register_sidecall_handler(kif::tilemux::Sidecalls::ActCtrl, activity_ctrl).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Map, map).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Translate, translate).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::SetQuota, set_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::RemoveQuotas, remove_quotas).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::ResetStats, reset_stats).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::GetQuota, get_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::DeriveQuota, derive_quota).ok();
}
