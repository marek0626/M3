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
use base::tcu;
use base::{build_vmsg, verror};

use thread::{Downgradable, TempRc, Upgradable};

use crate::cap::{Capability, KMemObject, MGateObject, SGateObject, ServObject, TileObject};
use crate::mem;
use crate::syscalls::{get_request, reply_success, try_upgrade_kobj};
use crate::tiles::{Activity, DeriveSrv};

#[inline(never)]
pub fn derive_tile_async(act: TempRc<Activity>) -> Result<(), VerboseError> {
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

    let tile: TempRc<TileObject> = act.get_kobj(r.tile)?;
    let act_id = act.id();
    let act_weak = act.downgrade_asyn();

    let tile_weak = tile.clone().downgrade_asyn();
    let tile_new = TileObject::derive_async(tile, r.eps, r.time, r.pts)?;
    let tile_new_clone = tile_new.clone();

    let act = match try {
        let cap = Capability::new(r.dst, tile_new);

        // TODO we will leak the quota object in TileMux if this fails
        let act = try_upgrade_kobj(act_weak, kif::INVALID_SEL)?;
        try_cap_insert!(act.obj_caps().borrow_mut().insert_as_child(cap, r.tile));

        act
    } {
        Ok(a) => a,
        Err(e) => {
            if let Some(tile) = tile_weak.upgrade() {
                TileObject::revoke_async(&tile_new_clone, tile, act_id);
            };
            return Err(e);
        },
    };

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_kmem(act: TempRc<Activity>) -> Result<(), VerboseError> {
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

    let kmem: TempRc<KMemObject> = act.get_kobj(r.kmem)?;
    if !kmem.has_quota(r.quota) {
        return Err(verror!(Code::NoSpace, "Insufficient quota"));
    }

    let cap = Capability::new(r.dst, KMemObject::new(r.quota));
    try_cap_insert!(act.obj_caps().borrow_mut().insert_as_child(cap, r.kmem));
    assert!(kmem.alloc(&act, r.kmem, r.quota));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_mem(act: TempRc<Activity>) -> Result<(), VerboseError> {
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

    let tact: TempRc<Activity> = act.get_kobj(r.act)?;

    let cap = {
        let act_caps = act.obj_caps().borrow();
        let mgate: TempRc<MGateObject> = act_caps.get_kobj(r.src)?;
        if r.offset.checked_add(r.size).is_none() || r.offset + r.size > mgate.size() || r.size == 0
        {
            return Err(verror!(Code::InvArgs, "Size or offset invalid"));
        }

        let addr = mgate.addr().raw() + r.offset;
        let new_mem = mem::Allocation::new(GlobAddr::new(addr), r.size);
        let mgate_obj = MGateObject::new(new_mem, r.perms & mgate.perms(), true);
        Capability::new(r.dst, mgate_obj)
    };

    try_cap_insert!(tact.obj_caps().borrow_mut().insert_as_child(cap, r.src));

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_srv_req(act: TempRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveSrvReq = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_srv_req(dst_srv={}, dst_sgate={}, srv={}, sessions={}, event={})",
        r.dst_srv,
        r.dst_sgate,
        r.srv,
        r.sessions,
        r.event
    );

    if r.sessions == 0 {
        return Err(verror!(Code::InvArgs, "Invalid session count"));
    }

    let srv: TempRc<ServObject> = act.get_kobj(r.srv)?;

    act.start_derive(DeriveSrv {
        src_srv: r.srv,
        dst_srv: r.dst_srv,
        dst_sgate: r.dst_sgate,
        event: r.event,
    })?;
    // if that fails, undo the start
    if let Err(e) = srv.set_derive_act(act.clone()) {
        act.finish_derive().unwrap();
        return Err(e.into());
    }

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

    if let Err(e) = ServObject::send(&srv, label, smsg) {
        srv.fetch_derive_act().unwrap();
        act.finish_derive().unwrap();
        return Err(e.into());
    }

    reply_success(&act);
    Ok(())
}

#[inline(never)]
pub fn derive_srv_fin(act: TempRc<Activity>) -> Result<(), VerboseError> {
    let msg = act.syscall();
    let r: syscalls::DeriveSrvFin = get_request(&msg)?;
    drop(msg);

    sysc_log!(
        act,
        "derive_srv_fin(srv={}, result={:?}, sgate={}, creator={})",
        r.srv,
        r.result,
        r.sgate,
        r.creator,
    );

    let srv: TempRc<ServObject> = act.get_kobj(r.srv)?;

    let der_act = srv.fetch_derive_act()?;
    let derive = der_act
        .finish_derive()
        .ok_or_else(|| Error::new(Code::InvState))?;

    let res = if r.result == Code::Success {
        // don't return here via ? but catch the error and always sent the upcall with the result
        let finish = || -> Result<(), VerboseError> {
            let src_srv: TempRc<ServObject> = der_act.get_kobj(derive.src_srv)?;

            let mut obj_caps = act.obj_caps().borrow_mut();
            let sgate_cap = obj_caps.get_mut(r.sgate)?;
            // ensure that this is actually a send gate
            sgate_cap.get::<TempRc<SGateObject>>()?;

            // pass sgate to calling activity
            try_cap_insert!(der_act.obj_caps().borrow_mut().obtain(
                derive.dst_sgate,
                sgate_cap,
                true
            ));

            // derive new service object and pass it to calling activity
            let derived_srv = src_srv.derive(r.creator);
            let cap = Capability::new(derive.dst_srv, derived_srv);
            try_cap_insert!(der_act
                .obj_caps()
                .borrow_mut()
                .insert_as_child(cap, derive.src_srv));
            Ok(())
        };
        match finish() {
            Err(e) => e.code(),
            Ok(_) => Code::Success,
        }
    }
    else {
        r.result
    };

    // notify calling activity via upcall
    der_act.upcall_derive_srv(derive.event, res);

    // return Err here to get the error print from the syscall handler
    if res != Code::Success {
        return Err(Error::new(res).into());
    }

    reply_success(&act);
    Ok(())
}
