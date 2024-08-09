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

use base::errors::{Code, Error, VerboseError};
use base::io::LogFlags;
use base::kif::{self, CapSel};
use base::log;
use base::mem;
use base::serialize::{Deserialize, M3Deserializer};
use base::tcu::{self, OwnedMessage};
use base::{build_vmsg, verror};

use thread::{Downgradable, TempRc, Upgradable, WeakRc};

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

macro_rules! try_cap_insert {
    ($e:expr) => {
        if let Err(e) = $e {
            Err(match e.code() {
                Code::NoSpace => verror!(e.code(), "Insufficient kernel memory quota"),
                Code::InvArgs => verror!(e.code(), "Selector already in use"),
                Code::ObjectGone => verror!(e.code(), "Activity is dead"),
                _ => panic!("unexpected capability insert error code"),
            })?;
        }
    };
}

mod create;
mod derive;
mod exchange;
mod misc;
mod tile;

fn try_upgrade_kobj<T>(weak: WeakRc<T>, sel: CapSel) -> Result<TempRc<T>, VerboseError> {
    weak.upgrade().ok_or_else(|| {
        if sel != kif::INVALID_SEL {
            verror!(
                Code::ObjectGone,
                "Kernel object (Selector {}) was revoked during async call",
                sel,
            )
        }
        else {
            verror!(
                Code::ObjectGone,
                "Kernel object was revoked during async call",
            )
        }
    })
}

fn send_reply(act: &TempRc<Activity>, rep: &mem::MsgBuf) {
    // Ignore errors as they should not occur with well-behaved applications.
    act.reply_syscall(rep).ok();
}

fn reply_result(act: &TempRc<Activity>, error: Code) {
    let mut rep_buf = mem::MsgBuf::borrow_def();
    build_vmsg!(rep_buf, kif::DefaultReply { error });
    send_reply(act, &rep_buf);
}

fn reply_success(act: &TempRc<Activity>) {
    reply_result(act, Code::Success);
}

fn get_request<'m, R: Deserialize<'m>>(msg: &'m OwnedMessage) -> Result<R, Error> {
    let mut de = M3Deserializer::new(msg.as_words());
    de.skip(1);
    de.pop()
}

fn sync_sys<F>(
    act: TempRc<Activity>,
    func: F,
) -> (Option<TempRc<Activity>>, Result<(), VerboseError>)
where
    F: FnOnce(&TempRc<Activity>) -> Result<(), VerboseError>,
{
    let res = func(&act);
    (Some(act), res)
}

fn async_sys<F>(
    act: TempRc<Activity>,
    func: F,
) -> (Option<TempRc<Activity>>, Result<(), VerboseError>)
where
    F: FnOnce(TempRc<Activity>) -> Result<(), VerboseError>,
{
    // only downgrade for async functions as this causes performance overhead
    let act_weak = act.clone().downgrade_asyn();
    let res = func(act);
    (act_weak.upgrade(), res)
}

// we actually call async functions indirectly over (a)sync_sys
#[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
pub fn handle_async(msg: tcu::OwnedMessage) {
    use kif::syscalls::Operation;

    let act = ActivityMng::activity(msg.header.label() as tcu::ActId).unwrap();
    let opcode = msg.as_words()[0];
    act.set_syscall(msg);

    // ignore complains about async aliases, because we just pass them to the wrapper functions
    // above. we could use a macro instead, but functions are more clean.
    #[cfg_attr(dylint_lib = "m3_lints", allow(async_alias))]
    let (act, res) = match opcode {
        o if o == Operation::CreateMGate.into() => sync_sys(act, create::create_mgate),
        o if o == Operation::CreateRGate.into() => sync_sys(act, create::create_rgate),
        o if o == Operation::CreateSGate.into() => sync_sys(act, create::create_sgate),
        o if o == Operation::CreateSrv.into() => sync_sys(act, create::create_srv),
        o if o == Operation::CreateSess.into() => sync_sys(act, create::create_sess),
        o if o == Operation::CreateAct.into() => async_sys(act, create::create_activity_async),
        o if o == Operation::CreateSem.into() => sync_sys(act, create::create_sem),
        o if o == Operation::CreateMap.into() => async_sys(act, create::create_map_async),

        o if o == Operation::DeriveTile.into() => async_sys(act, derive::derive_tile_async),
        o if o == Operation::DeriveMem.into() => sync_sys(act, derive::derive_mem),
        o if o == Operation::DeriveKMem.into() => sync_sys(act, derive::derive_kmem),
        o if o == Operation::DeriveSrvReq.into() => sync_sys(act, derive::derive_srv_req),
        o if o == Operation::DeriveSrvFin.into() => sync_sys(act, derive::derive_srv_fin),

        o if o == Operation::Exchange.into() => sync_sys(act, exchange::exchange),
        o if o == Operation::ExchangeSess.into() => {
            async_sys(act, exchange::exchange_over_sess_async)
        },
        o if o == Operation::Revoke.into() => async_sys(act, exchange::revoke_async),

        o if o == Operation::AllocEP.into() => async_sys(act, misc::alloc_ep_async),
        o if o == Operation::ActivateMGate.into() => sync_sys(act, misc::activate_mgate),
        o if o == Operation::ActivateRGate.into() => sync_sys(act, misc::activate_rgate),
        o if o == Operation::ActivateSGate.into() => async_sys(act, misc::activate_sgate_async),
        o if o == Operation::Invalidate.into() => sync_sys(act, misc::invalidate),
        o if o == Operation::MGateRegion.into() => sync_sys(act, misc::mgate_region),
        o if o == Operation::RGateBuffer.into() => sync_sys(act, misc::rgate_buffer),
        o if o == Operation::KMemQuota.into() => sync_sys(act, misc::kmem_quota),
        o if o == Operation::TileQuota.into() => async_sys(act, tile::tile_quota_async),
        o if o == Operation::TileSetQuota.into() => async_sys(act, tile::tile_set_quota_async),
        o if o == Operation::TileSetPMP.into() => sync_sys(act, tile::tile_set_pmp),
        o if o == Operation::TileReset.into() => async_sys(act, tile::tile_reset_async),
        o if o == Operation::TileInfo.into() => sync_sys(act, tile::tile_info),
        o if o == Operation::TileMem.into() => sync_sys(act, tile::tile_mem),
        o if o == Operation::GetSess.into() => sync_sys(act, misc::get_sess),
        o if o == Operation::SemCtrl.into() => async_sys(act, misc::sem_ctrl_async),
        o if o == Operation::ActCtrl.into() => async_sys(act, misc::activity_ctrl_async),
        o if o == Operation::ActWait.into() => async_sys(act, misc::activity_wait_async),

        o if o == Operation::ResetStats.into() => sync_sys(act, misc::reset_stats),
        o if o == Operation::Noop.into() => sync_sys(act, misc::noop),

        _ => panic!("Unexpected operation: {}", opcode),
    };

    if let Err(e) = res {
        if let Some(act) = act {
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

            reply_result(&act, e.code());
        }
    }
}
