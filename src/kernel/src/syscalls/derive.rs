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
use base::col::ToString;
use base::errors::{Code, Error, VerboseError};
use base::io::LogFlags;
use base::kif::{self, syscalls};
use base::log;
use base::mem::{GlobAddr, MsgBuf};
use base::rc::Rc;
use base::serialize::M3Deserializer;
use base::tcu;

use crate::cap::{Capability, KObject};
use crate::cap::{EPQuota, KMemObject, MGateObject, ServObject, TileObject};
use crate::com::Service;
use crate::mem;
use crate::syscalls::{check_unused, get_request, reply_success, try_upgrade_kobj};
use crate::tiles::{tilemng, Activity, TileMux};

#[inline(never)]
pub fn derive_tile_async(
    act: &Rc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::DeriveTile = get_request(msg)?;
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

    let tile = get_kobj!(act, r.tile, Tile);
    let tile_id = tile.tile();

    let ep_quota = if let Some(eps) = r.eps {
        if !tile.has_quota(eps) {
            sysc_err!(Code::NoSpace, "Insufficient EPs");
        }
        tile.alloc(eps);

        EPQuota::new(eps)
    }
    else {
        tile.ep_quota().clone()
    };

    let (time_id, pt_id) = if r.time.is_some() || r.pts.is_some() {
        let tilemux = tilemng::tilemux(tile_id);
        let time_quota_id = tile.time_quota_id();
        let pt_quota_id = tile.pt_quota_id();
        let tile_weak = tile.downgrade();

        let res = TileMux::derive_quota_async(tilemux, time_quota_id, pt_quota_id, r.time, r.pts);

        let tile = try_upgrade_kobj(tile_weak, r.tile)?;

        match res {
            Err(e) => {
                if let Some(eps) = r.eps {
                    tile.free(eps);
                }
                return Err(VerboseError::from(e));
            },
            Ok(v) => v,
        }
    }
    else {
        (tile.time_quota_id(), tile.pt_quota_id())
    };

    let cap = Capability::new(
        r.dst,
        KObject::Tile(TileObject::new(tile_id, ep_quota, time_id, pt_id, true)),
    );
    // TODO we will leak the quota object in TileMux if this fails
    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.tile));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn derive_kmem(act: &Rc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::DeriveKMem = get_request(msg)?;
    sysc_log!(
        act,
        "derive_kmem(kmem={}, dst={}, quota={:#x})",
        r.kmem,
        r.dst,
        r.quota
    );

    check_unused(&act.obj_caps().borrow(), r.dst)?;

    let kmem = get_kobj!(act, r.kmem, KMem);
    if !kmem.has_quota(r.quota) {
        sysc_err!(Code::NoSpace, "Insufficient quota");
    }

    let cap = Capability::new(r.dst, KObject::KMem(KMemObject::new(r.quota)));
    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.kmem));
    assert!(kmem.alloc(act, r.kmem, r.quota));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn derive_mem(act: &Rc<Activity>, msg: &mut tcu::OwnedMessage) -> Result<(), VerboseError> {
    let r: syscalls::DeriveMem = get_request(msg)?;
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

    let tact = get_kobj!(act, r.act, Activity);
    check_unused(&tact.obj_caps().borrow(), r.dst)?;

    let cap = {
        let act_caps = act.obj_caps().borrow();
        let mgate = get_kobj_ref!(act_caps, r.src, MGate);
        if r.offset.checked_add(r.size).is_none() || r.offset + r.size > mgate.size() || r.size == 0
        {
            sysc_err!(Code::InvArgs, "Size or offset invalid");
        }

        let addr = mgate.addr().raw() + r.offset;
        let new_mem = mem::Allocation::new(GlobAddr::new(addr), r.size);
        let mgate_obj = MGateObject::new(new_mem, r.perms & mgate.perms(), true);
        Capability::new(r.dst, KObject::MGate(mgate_obj))
    };

    try_kmem_quota!(tact.obj_caps().borrow_mut().insert_as_child(cap, r.src));

    reply_success(msg);
    Ok(())
}

#[inline(never)]
pub fn derive_srv_async(
    act: &Rc<Activity>,
    msg: &mut tcu::OwnedMessage,
) -> Result<(), VerboseError> {
    let r: syscalls::DeriveSrv = get_request(msg)?;
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
        sysc_err!(Code::InvArgs, "Invalid session count");
    }

    let srv = get_kobj!(act, r.srv, Serv);

    // everything worked, send the reply
    reply_success(msg);

    let mut smsg = MsgBuf::borrow_def();
    build_vmsg!(smsg, kif::service::Request::DeriveCrt {
        sessions: r.sessions
    });

    let label = srv.creator() as tcu::Label;
    log!(
        LogFlags::KernServ,
        "Sending derive_crt(sessions={}) to service {} with creator {}",
        r.sessions,
        srv.service().name(),
        label,
    );

    let srv_weak = srv.clone().downgrade();
    let res = Service::send_receive_async(srv, label, smsg);

    let srv = try_upgrade_kobj(srv_weak, r.srv)?;
    let res = match res {
        Err(e) => {
            sysc_log!(
                act,
                "Service {} unreachable: {:?}",
                srv.service().name(),
                e.code()
            );
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
                    let serv_act = srv.service().activity();
                    let mut serv_caps = serv_act.obj_caps().borrow_mut();
                    let src_cap = serv_caps.get_mut(reply.sgate_sel);
                    match src_cap {
                        None => {
                            sysc_log!(act, "Service gave invalid SendGate cap {}", reply.sgate_sel)
                        },
                        Some(c) => try_kmem_quota!(act.obj_caps().borrow_mut().obtain(
                            r.dst_sgate,
                            c,
                            true
                        )),
                    }

                    // derive new service object
                    let cap = Capability::new(
                        r.dst_srv,
                        KObject::Serv(ServObject::new(srv.service().clone(), false, reply.creator)),
                    );
                    try_kmem_quota!(act.obj_caps().borrow_mut().insert_as_child(cap, r.srv));
                    Ok(())
                },
                err => {
                    sysc_log!(
                        act,
                        "Server {} denied derive: {:?}",
                        srv.service().name(),
                        err
                    );
                    Err(Error::new(err))
                },
            }
        },
    };

    act.upcall_derive_srv(r.event, res);
    Ok(())
}
