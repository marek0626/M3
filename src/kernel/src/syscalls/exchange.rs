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

use anyhow::anyhow;

use base::build_vmsg;
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::{service, syscalls, CapRngDesc, CapType, INVALID_SEL, SEL_ACT};
use base::mem::MsgBuf;
use base::serialize::M3Deserializer;
use base::tcu;
use base::{format, log};

use thread::{Downgradable, TempRc, Upgradable};

use crate::cap::{ServObject, SessObject};
use crate::syscalls::{get_request, reply_success, send_reply, try_upgrade_kobj};
use crate::tiles::Activity;

fn do_exchange(
    act1: &TempRc<Activity>,
    act2: &TempRc<Activity>,
    c1: &CapRngDesc,
    c2: &CapRngDesc,
    obtain: bool,
) -> anyhow::Result<()> {
    let src = if obtain { act2 } else { act1 };
    let dst = if obtain { act1 } else { act2 };
    let src_rng = if obtain { c2 } else { c1 };
    let dst_rng = if obtain { c1 } else { c2 };

    if act1.id() == act2.id() {
        return Err(anyhow!(Error::new(Code::InvArgs)).context("Cap exchange with same Activity"));
    }
    if c1.cap_type() != c2.cap_type() {
        return Err(anyhow!(Error::new(Code::InvArgs)).context(format!(
            "Cap types differ ({:?} vs {:?})",
            c1.cap_type(),
            c2.cap_type(),
        )));
    }
    if (obtain && c2.count() > c1.count()) || (!obtain && c2.count() != c1.count()) {
        return Err(anyhow!(Error::new(Code::InvArgs)).context(format!(
            "Cap counts differ ({} vs {})",
            c2.count(),
            c1.count(),
        )));
    }

    // No TOCTOU as we do not have an async call in-between.
    if !dst.obj_caps().borrow().range_unused(dst_rng) {
        return Err(
            anyhow!(Error::new(Code::InvArgs)).context("Destination selectors already in use")
        );
    }

    for i in 0..c2.count() {
        let src_sel = src_rng.start() + i;
        let dst_sel = dst_rng.start() + i;
        let mut obj_caps_ref = src.obj_caps().borrow_mut();
        let src_cap = obj_caps_ref.try_get_mut(src_sel);
        let result = src_cap.map(|c| dst.obj_caps().borrow_mut().obtain(dst_sel, c));
        // Abort early on error but do no cleanup.
        if let Some(Err(e)) = result {
            return Err(e);
        }
    }

    Ok(())
}

#[inline(never)]
pub fn exchange(act: &TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::Exchange = get_request(&msg)?;
    drop(msg);

    let other_crd = CapRngDesc::new(r.own.cap_type(), r.other, r.own.count()).map_err(|e| {
        anyhow!(e).context(format!("Invalid cap range {}:{}", r.other, r.own.count()))
    })?;

    sysc_log!(
        act,
        "exchange(act={}, own={}, other={}, obtain={})",
        r.act,
        r.own,
        other_crd,
        r.obtain
    );

    let actcap: TempRc<Activity> = act.get_kobj(r.act)?;
    do_exchange(act, &actcap, &r.own, &other_crd, r.obtain)?;

    reply_success(act);
    Ok(())
}

#[inline(never)]
pub fn exchange_over_sess_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::ExchangeSess = get_request(&msg)?;
    drop(msg);

    let name = if r.obtain { "obtain" } else { "delegate" };
    sysc_log!(
        act,
        "{}(act={}, sess={}, crd={})",
        name,
        r.act,
        r.sess,
        r.crd
    );

    let sess: TempRc<SessObject> = act.get_kobj(r.sess)?;

    let mut smsg = MsgBuf::borrow_def();
    let data = service::ExchangeData {
        caps: r.crd,
        args: r.args,
    };
    build_vmsg!(
        smsg,
        if r.obtain {
            service::Request::Obtain {
                sid: sess.ident(),
                data,
            }
        }
        else {
            service::Request::Delegate {
                sid: sess.ident(),
                data,
            }
        }
    );

    let serv = sess
        .service()
        .ok_or_else(|| anyhow!(Error::new(Code::ObjectGone)).context("Service was destroyed"))?;
    let label = sess.creator() as tcu::Label;

    log!(
        LogFlags::KernServ,
        "Sending {}(sess={:#x}, caps={}, args={}B) to service {} with creator {}",
        name,
        sess.ident(),
        r.crd.count(),
        r.args.bytes,
        serv.name(),
        label,
    );
    drop(sess);

    let serv_weak = serv.clone().downgrade_asyn();
    let act_weak = act.downgrade_asyn();
    let res = ServObject::send_receive_async(serv, label, smsg);
    let act = try_upgrade_kobj(act_weak, INVALID_SEL)?;
    let serv = try_upgrade_kobj(serv_weak, INVALID_SEL)?;

    let rmsg = match res {
        Ok(rmsg) => rmsg,
        Err(e) => return Err(e.context(format!("Service {} unreachable", serv.name()))),
    };

    let mut de = M3Deserializer::new(rmsg.as_words());
    let err: Code = de
        .pop()
        .map_err(|e| anyhow!(e).context("Invalid server response"))?;
    match err {
        Code::Success => {},
        err => {
            return Err(anyhow!(Error::new(err))
                .context(format!("Server {} denied cap exchange", serv.name())))
        },
    }

    let reply: service::ExchangeReply = de
        .pop()
        .map_err(|e| anyhow!(e).context("Invalid server response"))?;

    sysc_log!(
        act,
        "{} continue with res={:?}, srv_crd={}",
        name,
        err,
        reply.data.caps
    );

    let actcap: TempRc<Activity> = act.get_kobj(r.act)?;
    do_exchange(
        &actcap,
        &serv.server_act(),
        &r.crd,
        &reply.data.caps,
        r.obtain,
    )?;

    let mut kreply = MsgBuf::borrow_def();
    build_vmsg!(kreply, Code::Success, syscalls::ExchangeSessReply {
        args: reply.data.args,
    });
    send_reply(&act, &kreply);

    Ok(())
}

#[inline(never)]
pub fn revoke_async(act: TempRc<Activity>) -> anyhow::Result<()> {
    let msg = act.syscall();
    let r: syscalls::Revoke = get_request(&msg)?;
    drop(msg);

    sysc_log!(act, "revoke(act={}, crd={}, own={})", r.act, r.crd, r.own);

    if r.crd.cap_type() == CapType::Object && r.crd.start() <= SEL_ACT {
        return Err(anyhow!(Error::new(Code::InvArgs)).context("Cap 0, 1 and 2 are not revokeable"));
    }

    let actcap = {
        let actcap: TempRc<Activity> = act.get_kobj(r.act)?;
        // TODO this does not work; we probably need to do the revoke in two phases: 1. remove all
        // links and collect the objects to destroy (sync) and 2. destroy the objects (async)
        unsafe { TempRc::into_strong_unchecked(actcap) }
    };

    let act_id = act.id();

    let act_weak = act.downgrade_asyn();

    actcap.revoke_async(r.crd, r.own, act_id);

    if let Some(act) = act_weak.upgrade() {
        reply_success(&act);
    }
    Ok(())
}
