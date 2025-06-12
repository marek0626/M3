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

use base::cell::{Ref, StaticRefCell};
use base::cfg;
use base::col::Vec;
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{GlobAddr, MsgBuf, VirtAddr, VirtAddrRaw};
use base::serialize::{Deserialize, M3Deserializer};
use base::tcu::{self, TCU};
use base::time::TimeDuration;

use crate::{activities, pex_env};
use mux::sidecalls::*;
use mux::{helper, sendqueue};

fn side_rbuf_addr() -> VirtAddr {
    crate::pex_env().tile_desc.rbuf_mux_space().0 + cfg::KPEX_RBUF_SIZE as VirtAddrRaw
}

fn info(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::info()",);
    Ok((kif::syscalls::MuxType::Unimux.into(), 0))
}

fn activity_init(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ActInit = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::activity_init(act={}, eps_start={})",
        r.act_id,
        r.eps_start,
    );

    // We "steal" these resources.
    activities::set_user(r.act_id, r.eps_start);
    Ok((0, 0))
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
            activities::user().start();
            Ok((0, 0))
        },

        kif::tilemux::ActivityOp::Stop => {
            // mark it as blocked to idle instead of returning to the app
            activities::user().set_blocked(true);
            Ok((0, 0))
        },

        _ => Ok((0, 0)),
    }
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

    Ok((0, 0))
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

    Ok((1, 1))
}

fn shutdown(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::shutdown()",);

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

fn get_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::GetQuota = get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::get_quota(time={}, pts={})",
        r.time,
        r.pts
    );

    Ok((1 << 32 | 1, 1 << 32 | 1))
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

    match r.virt {
        cfg::ENV_START => Ok((GlobAddr::new(cfg::ENV_START.as_goff()).raw(), 0)),
        cfg::RBUF_STD_ADDR => Ok((
            GlobAddr::new(pex_env().tile_desc.rbuf_std_space().0.as_goff()).raw(),
            0,
        )),
        _ => Err(Error::new(Code::NotSup)),
    }
}

fn go_away(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "Not supported! Go away kernel!");
    Ok((0, 0))
}

// Sidecalls are messages from the kernel to do many kinds of things at the basic resource level, such as changing mappings, managing an activity, etc.
fn handle_sidecall(msg: &'static tcu::Message) {
    let mut de = M3Deserializer::new(msg.as_words());

    let op: kif::tilemux::Sidecalls = de.pop().unwrap();
    let res = if let Some(handler) = mux::sidecalls::find_handler(op) {
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

pub fn check() {
    let mut our = activities::our();
    let _cmd_saved = helper::TCUGuard::new();

    loop {
        // change to our activity
        let old_act = tcu::TCU::xchg_activity(our.activity_reg()).unwrap();
        if let Some(mut old) = activities::try_cur() {
            activities::get_mut(old).unwrap().set_activity_reg(old_act);
        }

        // if the SEP is still frozen, it means that the kernel just initialized our tile and these
        // EPs, so unfreeze them. Note that the kernel does not configure TMSIDE_REP (rosa did that)
        // and thus it does not need to be unfrozen.
        #[cfg(M3_TARGET = "gem5")]
        if TCU::is_frozen(tcu::KPEX_REP) {
            let tile_desc = crate::pex_env().tile_desc;
            TCU::check_recv_ep(
                tcu::KPEX_REP,
                tile_desc.rbuf_mux_space().0.as_phys(tile_desc),
                1 << cfg::KPEX_RBUF_ORD,
                false,
            )
            .expect("KPEX_REP not sane");

            TCU::unfreeze(tcu::KPEX_REP).unwrap();
            TCU::unfreeze(tcu::KPEX_SEP).unwrap();
        }

        if let Some(msg_off) = tcu::TCU::fetch_msg(tcu::TMSIDE_REP) {
            let msg = tcu::TCU::offset_to_msg(side_rbuf_addr(), msg_off);
            handle_sidecall(msg);
        }

        // check if the kernel answered a request from us
        sendqueue::check_replies();

        // change back to old activity

        let new_act = activities::try_cur().unwrap_or(old_act);
        our.set_activity_reg(tcu::TCU::xchg_activity(new_act).unwrap());
        // if no events arrived in the meantime, we're done
        if !our.has_msgs() {
            break;
        }
    }
}

pub fn basic_handlers_init() {
    register_sidecall_handler(kif::tilemux::Sidecalls::ActInit, activity_init).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::ActCtrl, activity_ctrl).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::RemMsgs, go_away).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::EPInval, go_away).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Shutdown, shutdown).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Translate, translate).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Info, info).ok();

    register_sidecall_handler(kif::tilemux::Sidecalls::GetQuota, get_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::SetQuota, set_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::DeriveQuota, derive_quota).ok();
}
