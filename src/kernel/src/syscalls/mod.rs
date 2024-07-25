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

use thread::{AsyncRc, AsyncWeak};

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
                _ => panic!("unexpected capability insert error code: {:?}", e.code()),
            })?;
        }
    };
}

mod create;
mod derive;
mod exchange;
mod misc;
mod tile;

fn try_upgrade_kobj<T>(weak: AsyncWeak<T>, sel: CapSel) -> Result<AsyncRc<T>, VerboseError> {
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

fn send_reply(act: &AsyncRc<Activity>, rep: &mem::MsgBuf) {
    // Ignore errors as they should not occur with well-behaved applications.
    act.reply_syscall(rep).ok();
}

fn reply_result(act: &AsyncRc<Activity>, error: Code) {
    let mut rep_buf = mem::MsgBuf::borrow_def();
    build_vmsg!(rep_buf, kif::DefaultReply { error });
    send_reply(act, &rep_buf);
}

fn reply_success(act: &AsyncRc<Activity>) {
    reply_result(act, Code::Success);
}

fn get_request<'m, R: Deserialize<'m>>(msg: &'m OwnedMessage) -> Result<R, Error> {
    let mut de = M3Deserializer::new(msg.as_words());
    de.skip(1);
    de.pop()
}

pub fn handle_async(msg: tcu::OwnedMessage) {
    use kif::syscalls::Operation;

    let act = ActivityMng::activity(msg.header.label() as tcu::ActId).unwrap();
    let act_weak = act.clone().downgrade();
    let opcode = msg.as_words()[0];
    act.set_syscall(msg);

    let res = match opcode {
        o if o == Operation::CreateMGate.into() => create::create_mgate(act),
        o if o == Operation::CreateRGate.into() => create::create_rgate(act),
        o if o == Operation::CreateSGate.into() => create::create_sgate(act),
        o if o == Operation::CreateSrv.into() => create::create_srv(act),
        o if o == Operation::CreateSess.into() => create::create_sess(act),
        o if o == Operation::CreateAct.into() => create::create_activity_async(act),
        o if o == Operation::CreateSem.into() => create::create_sem(act),
        o if o == Operation::CreateMap.into() => create::create_map_async(act),

        o if o == Operation::DeriveTile.into() => derive::derive_tile_async(act),
        o if o == Operation::DeriveMem.into() => derive::derive_mem(act),
        o if o == Operation::DeriveKMem.into() => derive::derive_kmem(act),
        o if o == Operation::DeriveSrvReq.into() => derive::derive_srv_req(act),
        o if o == Operation::DeriveSrvFin.into() => derive::derive_srv_fin(act),

        o if o == Operation::Exchange.into() => exchange::exchange(act),
        o if o == Operation::ExchangeSess.into() => exchange::exchange_over_sess_async(act),
        o if o == Operation::Revoke.into() => exchange::revoke_async(act),

        o if o == Operation::AllocEP.into() => misc::alloc_ep_async(act),
        o if o == Operation::ActivateMGate.into() => misc::activate_mgate(act),
        o if o == Operation::ActivateRGate.into() => misc::activate_rgate(act),
        o if o == Operation::ActivateSGate.into() => misc::activate_sgate_async(act),
        o if o == Operation::Invalidate.into() => misc::invalidate(act),
        o if o == Operation::MGateRegion.into() => misc::mgate_region(act),
        o if o == Operation::RGateBuffer.into() => misc::rgate_buffer(act),
        o if o == Operation::KMemQuota.into() => misc::kmem_quota(act),
        o if o == Operation::TileQuota.into() => tile::tile_quota_async(act),
        o if o == Operation::TileSetQuota.into() => tile::tile_set_quota_async(act),
        o if o == Operation::TileSetPMP.into() => tile::tile_set_pmp(act),
        o if o == Operation::TileReset.into() => tile::tile_reset_async(act),
        o if o == Operation::TileInfo.into() => tile::tile_info(act),
        o if o == Operation::TileMem.into() => tile::tile_mem(act),
        o if o == Operation::GetSess.into() => misc::get_sess(act),
        o if o == Operation::SemCtrl.into() => misc::sem_ctrl_async(act),
        o if o == Operation::ActCtrl.into() => misc::activity_ctrl_async(act),
        o if o == Operation::ActWait.into() => misc::activity_wait_async(act),

        o if o == Operation::ResetStats.into() => misc::reset_stats(act),
        o if o == Operation::Noop.into() => misc::noop(act),

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

            reply_result(&act, e.code());
        }
    }
}
