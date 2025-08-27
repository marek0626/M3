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

use base::cell::StaticCell;
use base::cfg;
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{GlobAddr, GlobOff, MsgBuf, VirtAddr, VirtAddrRaw};
use base::serialize::M3Deserializer;
use base::tcu::{self, GenId, TileId};
use mux::sendqueue;
use mux::sidecalls;

use crate::{_shutdown, hdl, pex_env};

pub type AddExRegHandler =
    fn(TileId, usize, TileId, GenId, GlobOff, GlobOff, kif::Perm, bool) -> Result<(), Error>;
pub type RemExRegHandler = fn(TileId, usize) -> Result<(), Error>;

static EXREG_HDL: StaticCell<Option<(AddExRegHandler, RemExRegHandler)>> = StaticCell::new(None);

fn side_rbuf_addr() -> VirtAddr {
    crate::pex_env().tile_desc.rbuf_mux_space().0 + cfg::KPEX_RBUF_SIZE as VirtAddrRaw
}

fn info(_msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "sidecall::info()",);
    Ok((kif::syscalls::MuxType::Unimux.into(), 0))
}

fn activity_init(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ActInit = sidecalls::get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::activity_init(act={}, eps_start={})",
        r.act_id,
        r.eps_start,
    );

    hdl::user_init(r.act_id);
    Ok((0, 0))
}

fn activity_ctrl(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ActivityCtrl = sidecalls::get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::activity_ctrl(act={}, op={:?})",
        r.act_id,
        r.act_op,
    );

    match r.act_op {
        kif::tilemux::ActivityOp::Start => {
            hdl::user_start();
            Ok((0, 0))
        },

        kif::tilemux::ActivityOp::Stop => {
            // mark it as blocked to idle instead of returning to the app
            hdl::user_block();
            Ok((0, 0))
        },
    }
}

fn set_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::SetQuota = sidecalls::get_request(msg)?;

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
    let r: kif::tilemux::DeriveQuota = sidecalls::get_request(msg)?;

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
    sidecalls::reply_msg(msg, &reply_buf);

    // call shutdown here directly after reply, so that we hopefully don't execute any code while
    // the kernel resets the tile. this is actually just a workaround for gem5, where we cannot
    // reset the core properly.
    unsafe {
        _shutdown();
    }
}

fn get_quota(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::GetQuota = sidecalls::get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::get_quota(time={}, pts={})",
        r.time,
        r.pts
    );

    Ok((1 << 32 | 1, 1 << 32 | 1))
}

fn translate(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::Translate = sidecalls::get_request(msg)?;

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

fn exreg_add(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ExRegAdd = sidecalls::get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::exreg_add(mtile={}, idx={}, utile={}, ugen={}, addr={:#x}, size={:#x}, perm={:?}, locked={})",
        r.mtile,
        r.idx,
        r.utile,
        r.ugen,
        r.addr,
        r.size,
        r.perm,
        r.locked,
    );

    if let Some((hdl, _)) = EXREG_HDL.get() {
        hdl(
            r.mtile, r.idx, r.utile, r.ugen, r.addr, r.size, r.perm, r.locked,
        )?;
    }

    Ok((0, 0))
}

fn exreg_rem(msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    let r: kif::tilemux::ExRegRem = sidecalls::get_request(msg)?;

    log!(
        LogFlags::MuxSideCalls,
        "sidecall::exreg_rem(mtile={}, idx={})",
        r.mtile,
        r.idx
    );

    if let Some((_, hdl)) = EXREG_HDL.get() {
        hdl(r.mtile, r.idx)?;
    }

    Ok((0, 0))
}

fn go_away(_msg: &'static tcu::Message) -> Result<(u64, u64), Error> {
    log!(LogFlags::MuxSideCalls, "Not supported! Go away kernel!");
    Ok((0, 0))
}

// Sidecalls are messages from the kernel to do many kinds of things at the basic resource level, such as changing mappings, managing an activity, etc.
fn handle_sidecall(msg: &'static tcu::Message) {
    let mut de = M3Deserializer::new(msg.as_words());

    let op: kif::tilemux::Sidecalls = de.pop().unwrap();
    let res = if let Some(handler) = sidecalls::find_handler(op) {
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
    sidecalls::reply_msg(msg, &reply_buf);
}

fn do_check() -> bool {
    // if the SEP is still frozen, it means that the kernel just initialized our tile and these
    // EPs, so unfreeze them. Note that the kernel does not configure TMSIDE_REP (rosa did that)
    // and thus it does not need to be unfrozen.
    #[cfg(any(M3_TARGET = "gem5", M3_TARGET = "hw"))]
    if tcu::TCU::is_frozen(tcu::KPEX_REP) {
        let tile_desc = crate::pex_env().tile_desc;
        tcu::TCU::check_recv_ep(
            tcu::KPEX_REP,
            tile_desc.rbuf_mux_space().0.as_phys(tile_desc),
            1 << cfg::KPEX_RBUF_ORD,
            false,
        )
        .expect("KPEX_REP not sane");

        tcu::TCU::unfreeze(tcu::KPEX_REP).unwrap();
        tcu::TCU::unfreeze(tcu::KPEX_SEP).unwrap();
    }

    let handled = if let Some(msg_off) = tcu::TCU::fetch_msg(tcu::TMSIDE_REP) {
        let msg = tcu::TCU::offset_to_msg(side_rbuf_addr(), msg_off);
        handle_sidecall(msg);
        true
    }
    else {
        false
    };

    // check if the kernel answered a request from us
    sendqueue::check_replies();

    handled
}

pub fn check() {
    hdl::handle_sidecalls(do_check);
}

pub fn basic_handlers_init() {
    use sidecalls::register_sidecall_handler;
    register_sidecall_handler(kif::tilemux::Sidecalls::ActInit, activity_init).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::ActCtrl, activity_ctrl).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::RemMsgs, go_away).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::EPInval, go_away).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::ExRegAdd, exreg_add).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::ExRegRem, exreg_rem).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Shutdown, shutdown).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Translate, translate).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::Info, info).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::GetQuota, get_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::SetQuota, set_quota).ok();
    register_sidecall_handler(kif::tilemux::Sidecalls::DeriveQuota, derive_quota).ok();
}

pub fn reg_exreg_handler(add: AddExRegHandler, remove: RemExRegHandler) {
    assert!(EXREG_HDL.get().is_none());
    EXREG_HDL.set(Some((add, remove)));
}
