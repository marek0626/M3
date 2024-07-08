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

use base::col::ToString;
use base::errors::{Code, Error, VerboseError};
use base::io::LogFlags;
use base::kif::{self, CapSel};
use base::log;
use base::mem;
use base::serialize::{Deserialize, M3Deserializer};
use base::tcu;
use base::{build_vmsg, format};

use thread::{AsyncRc, AsyncWeak};

use crate::cap::{CapTable, Capability, KObject};
use crate::tiles::{Activity, ActivityMng};

#[macro_export]
macro_rules! sysc_log {
    ($act:expr, $fmt:tt, $($args:tt)*) => (
        $crate::log!(
            base::io::LogFlags::KernSysc,
            concat!("{}:{}@{}: syscall::", $fmt),
            $act.id(), $act.name(), $act.tile_id(), $($args)*
        )
    )
}

#[macro_export]
macro_rules! sysc_err {
    ($e:expr, $fmt:tt) => ({
        return Err(base::errors::VerboseError::new($e, $fmt.to_string()));
    });
    ($e:expr, $fmt:tt, $($args:tt)*) => ({
        return Err(base::errors::VerboseError::new($e, base::format!($fmt, $($args)*)));
    });
}

macro_rules! try_kmem_quota {
    ($e:expr) => {
        if let Err(e) = $e {
            sysc_err!(e.code(), "Insufficient kernel memory quota");
        }
    };
}

mod create;
mod derive;
mod exchange;
mod misc;
mod tile;

fn check_unused(tbl: &CapTable, sel: CapSel) -> Result<(), VerboseError> {
    if !tbl.unused(sel) {
        sysc_err!(Code::InvArgs, "Selector {} already in use", sel);
    }
    Ok(())
}

fn get_kobj<T>(act: &AsyncRc<Activity>, sel: kif::CapSel) -> Result<T, VerboseError>
where
    T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
{
    let table = act.obj_caps().borrow();
    get_kobj_ref(&table, sel)
}

fn get_cap(table: &CapTable, sel: kif::CapSel) -> Result<&Capability, VerboseError> {
    match table.get(sel) {
        Some(c) => Ok(c),
        None => sysc_err!(Code::InvArgs, "Invalid capability"),
    }
}

fn get_kobj_ref<T>(table: &CapTable, sel: kif::CapSel) -> Result<T, VerboseError>
where
    T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
{
    let cap = get_cap(table, sel)?;
    cap_to_kobj(cap)
}

fn cap_to_kobj<T>(cap: &Capability) -> Result<T, VerboseError>
where
    T: for<'a> TryFrom<&'a KObject, Error = VerboseError>,
{
    // safety: we directly turn it into a KObjectOwnedRef here, so that it's okay
    unsafe { cap.get() }.try_into()
}

fn try_upgrade_kobj<T>(weak: AsyncWeak<T>, sel: CapSel) -> Result<AsyncRc<T>, VerboseError> {
    weak.upgrade().ok_or_else(|| {
        VerboseError::new(
            Code::ObjectGone,
            if sel != kif::INVALID_SEL {
                format!(
                    "Kernel object (Selector {}) was revoked during async call",
                    sel
                )
            }
            else {
                "Kernel object was revoked during async call".to_string()
            },
        )
    })
}

fn send_reply(msg: &mut tcu::OwnedMessage, rep: &mem::MsgBuf) {
    // Ignore errors as they should not occur with well-behaved applications.
    msg.reply(rep).ok();
}

fn reply_result(msg: &mut tcu::OwnedMessage, error: Code) {
    let mut rep_buf = mem::MsgBuf::borrow_def();
    build_vmsg!(rep_buf, kif::DefaultReply { error });
    send_reply(msg, &rep_buf);
}

fn reply_success(msg: &mut tcu::OwnedMessage) {
    reply_result(msg, Code::Success);
}

fn get_request<'m, R: Deserialize<'m>>(msg: &'m tcu::Message) -> Result<R, Error> {
    let mut de = M3Deserializer::new(msg.as_words());
    de.skip(1);
    de.pop()
}

pub fn handle_async(mut msg: tcu::OwnedMessage) {
    let msg = &mut msg;
    let act = ActivityMng::activity(msg.header.label() as tcu::ActId).unwrap();
    let act_weak = act.clone().downgrade();

    use kif::syscalls::Operation;
    let opcode = msg.as_words()[0];
    let res = match opcode {
        o if o == Operation::CreateMGate.into() => create::create_mgate(act, msg),
        o if o == Operation::CreateRGate.into() => create::create_rgate(act, msg),
        o if o == Operation::CreateSGate.into() => create::create_sgate(act, msg),
        o if o == Operation::CreateSrv.into() => create::create_srv(act, msg),
        o if o == Operation::CreateSess.into() => create::create_sess(act, msg),
        o if o == Operation::CreateAct.into() => create::create_activity_async(act, msg),
        o if o == Operation::CreateSem.into() => create::create_sem(act, msg),
        o if o == Operation::CreateMap.into() => create::create_map_async(act, msg),

        o if o == Operation::DeriveTile.into() => derive::derive_tile_async(act, msg),
        o if o == Operation::DeriveMem.into() => derive::derive_mem(act, msg),
        o if o == Operation::DeriveKMem.into() => derive::derive_kmem(act, msg),
        o if o == Operation::DeriveSrv.into() => derive::derive_srv_async(act, msg),

        o if o == Operation::Exchange.into() => exchange::exchange(act, msg),
        o if o == Operation::ExchangeSess.into() => exchange::exchange_over_sess_async(act, msg),
        o if o == Operation::Revoke.into() => exchange::revoke_async(act, msg),

        o if o == Operation::AllocEP.into() => misc::alloc_ep_async(act, msg),
        o if o == Operation::ActivateMGate.into() => misc::activate_mgate(act, msg),
        o if o == Operation::ActivateRGate.into() => misc::activate_rgate(act, msg),
        o if o == Operation::ActivateSGate.into() => misc::activate_sgate_async(act, msg),
        o if o == Operation::Invalidate.into() => misc::invalidate(act, msg),
        o if o == Operation::MGateRegion.into() => misc::mgate_region(act, msg),
        o if o == Operation::RGateBuffer.into() => misc::rgate_buffer(act, msg),
        o if o == Operation::KMemQuota.into() => misc::kmem_quota(act, msg),
        o if o == Operation::TileQuota.into() => tile::tile_quota_async(act, msg),
        o if o == Operation::TileSetQuota.into() => tile::tile_set_quota_async(act, msg),
        o if o == Operation::TileSetPMP.into() => tile::tile_set_pmp(act, msg),
        o if o == Operation::TileReset.into() => tile::tile_reset_async(act, msg),
        o if o == Operation::TileInfo.into() => tile::tile_info(act, msg),
        o if o == Operation::TileMem.into() => tile::tile_mem(act, msg),
        o if o == Operation::GetSess.into() => misc::get_sess(act, msg),
        o if o == Operation::SemCtrl.into() => misc::sem_ctrl_async(act, msg),
        o if o == Operation::ActCtrl.into() => misc::activity_ctrl_async(act, msg),
        o if o == Operation::ActWait.into() => misc::activity_wait_async(act, msg),

        o if o == Operation::ResetStats.into() => misc::reset_stats(act, msg),
        o if o == Operation::Noop.into() => misc::noop(act, msg),

        _ => panic!("Unexpected operation: {}", opcode),
    };

    if let Err(e) = res {
        if let Some(act) = act_weak.upgrade() {
            log!(
                LogFlags::Error,
                "\x1B[37;41m{}:{}@{}: {:?} failed: {} ({:?})\x1B[0m",
                act.id(),
                act.name(),
                act.tile_id(),
                Operation::try_from(opcode),
                e.msg(),
                e.code()
            );

            reply_result(msg, e.code());
        }
    }
}
