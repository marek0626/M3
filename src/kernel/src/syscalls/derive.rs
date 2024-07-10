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
use base::kif::{self, syscalls};
use base::log;
use base::mem::{GlobAddr, MsgBuf};
use base::serialize::M3Deserializer;
use base::tcu;
use base::{build_vmsg, verror};

use thread::AsyncRc;

use crate::cap::{Capability, KMemObject, MGateObject, ServObject, TileObject};
use crate::mem;
use crate::syscalls::{check_unused, get_request, reply_success, try_upgrade_kobj};
use crate::tiles::Activity;

#[inline(never)]
pub fn derive_tile_async(act: AsyncRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveTile = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_tile(tile={}, dst={}, eps={:?}, time={:?}, pts={:?})",
        r.tile,
        r.dst,
        r.eps,
        r.time,
        r.pts,
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;

    let tile: AsyncRc<TileObject> = act.get_kobj(r.tile)?;
    let act_weak = act.downgrade();

    let tile_new = TileObject::derive_async(tile, r.eps, r.time, r.pts)?;
    let cap = Capability::new(r.dst, tile_new);

    // TODO we will leak the quota object in TileMux if this fails
    let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.tile));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_kmem(act: AsyncRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveKMem = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_kmem(kmem={}, dst={}, quota={:#x})",
        r.kmem,
        r.dst,
        r.quota
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;

    let kmem: AsyncRc<KMemObject> = act.get_kobj(r.kmem)?;
    if !kmem.has_quota(r.quota) {
        return Err(verror!(Code::NoSpace, "Insufficient quota"));
    }

    let cap = Capability::new(r.dst, KMemObject::new(r.quota));
    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.kmem));
    assert!(kmem.alloc(&act, r.kmem, r.quota));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_mem(act: AsyncRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveMem = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_mem(act={}, src={}, dst={}, size={:#x}, offset={:#x}, perms={:?})",
        r.act,
        r.src,
        r.dst,
        r.size,
        r.offset,
        r.perms
    );

    let tact: AsyncRc<Activity> = act.get_kobj(r.act)?;
    check_unused(&tact.obj_caps().borrow(), r.dst)?;

    let cap = {
        let act_caps = act.obj_caps().borrow();
        let mgate: AsyncRc<MGateObject> = act_caps.get_kobj(r.src)?;
        if r.offset.checked_add(r.size).is_none() || r.offset + r.size > mgate.size() || r.size == 0
        {
            return Err(verror!(Code::InvArgs, "Size or offset invalid"));
        }

        let addr = mgate.addr().raw() + r.offset;
        let new_mem = mem::Allocation::new(GlobAddr::new(addr), r.size);
        let mgate_obj = MGateObject::new(new_mem, r.perms & mgate.perms(), true);
        Capability::new(r.dst, mgate_obj)
    };

    try_kmem_quota!(tact.obj_caps().borrow_mut().insert_as_child(cap, r.src));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_srv_async(act: AsyncRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveSrv = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_srv(dst_srv={}, dst_sgate={}, srv={}, sessions={}, event={})",
        r.dst_srv,
        r.dst_sgate,
        r.srv,
        r.sessions,
        r.event
    );

    check_unused(&act.obj_caps().borrow(), r.dst_srv)?;
    check_unused(&act.obj_caps().borrow(), r.dst_sgate)?;

    if r.sessions == 0 {
        return Err(verror!(Code::InvArgs, "Invalid session count"));
    }

    let srv: AsyncRc<ServObject> = act.get_kobj(r.srv)?;

    // everything worked, send the reply
    reply_success(&act);

    let mut smsg = MsgBuf::borrow_def();
    build_vmsg!(smsg, kif::service::Request::DeriveCrt {
        sessions: r.sessions
    });

    let label = srv.creator() as tcu::Label;
    log!(
        LogFlags::KernServ,
        "Sending derive_crt(sessions={}) to service {} with creator {}",
        r.sessions,
        srv.name(),
        label,
    );

    let srv_weak = srv.clone().downgrade();
    let act_weak = act.downgrade();
    let res = ServObject::send_receive_async(srv, label, smsg);

    let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
    let srv = try_upgrade_kobj(srv_weak, r.srv)?;
    let res = match res {
        Err(e) => {
            sysc_log!(act, "Service {} unreachable: {:?}", srv.name(), e.code());
            Err(e)
        },

        Ok(rmsg) => {
            let mut de = M3Deserializer::new(rmsg.as_words());
            let err: Code = de.pop()?;
            match err {
                Code::Success => {
                    let reply: kif::service::DeriveCreatorReply = de.pop()?;

                    sysc_log!(act, "derive_srv continue with creator={}", reply.creator);

                    // obtain SendGate from server (do that first because it can fail)
                    let serv_act = srv.server_act();
                    let mut serv_caps = serv_act.obj_caps().borrow_mut();
                    let src_cap = serv_caps.get_mut(reply.sgate_sel);
                    match src_cap {
                        Err(_) => {
                            sysc_log!(act, "Service gave invalid SendGate cap {}", reply.sgate_sel)
                        },
                        Ok(c) => try_kmem_quota!(act.obj_caps().borrow_mut().obtain(
                            r.dst_sgate,
                            c,
                            true
                        )),
                    }

                    // derive new service object
                    let derived_srv = srv.derive(reply.creator);
                    let cap = Capability::new(r.dst_srv, derived_srv);
                    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.srv));
                    Ok(())
                },
                err => {
                    sysc_log!(act, "Server {} denied derive: {:?}", srv.name(), err);
                    Err(Error::new(err))
                },
            }
        },
    };

    act.upcall_derive_srv(r.event, res);
    Ok(())
}
