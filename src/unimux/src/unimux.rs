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

#![no_std]

#[cfg(any(M3_TARGET = "gem5", target_arch = "riscv64"))]
#[path = "preempt/mod.rs"]
mod hdl;

#[cfg(not(any(M3_TARGET = "gem5", target_arch = "riscv64")))]
#[path = "nopreempt/mod.rs"]
mod hdl;

mod sidecalls;

use base::cell::{Ref, StaticRefCell};
use base::cfg;
use base::env;
use base::errors::Code;
use base::io;
use base::kif::{self, TileAttr};
use base::mem::MsgBuf;
use base::tcu::{self, TCU};
use mux::sendqueue;

use core::ptr;

extern "C" {
    fn __m3_init_libc(argc: i32, argv: *const *const u8, envp: *const *const u8, tls: bool);
    fn _shutdown() -> !;
}

static TM_ENV: StaticRefCell<mux::TMEnv> = StaticRefCell::new(mux::TMEnv {
    tile_id: 0,
    org_tile_desc: kif::TileDesc::new_from(0),
    tile_desc: kif::TileDesc::new_from(0),
    platform: env::Platform::Gem5,
});

pub fn pex_env() -> Ref<'static, mux::TMEnv> {
    TM_ENV.borrow()
}

pub fn app_env() -> &'static mut env::BaseEnv {
    unsafe { &mut *(cfg::ENV_START.as_mut_ptr()) }
}

pub fn send_exit(act_id: tcu::ActId, status: Code) {
    let mut msg_buf = MsgBuf::borrow_def();
    base::build_vmsg!(msg_buf, kif::tilemux::Calls::Exit, kif::tilemux::Exit {
        act_id,
        status,
    });
    sendqueue::send(&msg_buf).unwrap();
}

#[no_mangle]
pub extern "C" fn abort() -> ! {
    unsafe {
        _shutdown();
    }
}

#[no_mangle]
pub extern "C" fn exit(code: u32) -> ! {
    if let Some(act_id) = hdl::user_id() {
        send_exit(act_id, Code::try_from(code).unwrap());
    }

    loop {
        sidecalls::check();
    }
}

pub fn check_sidecalls() {
    sidecalls::check();
}

extern "Rust" {
    fn env_run() -> !;
}

#[no_mangle]
pub extern "C" fn init() -> ! {
    // copy the environment from earlier stages if we are the RoT
    // (on hw the environment is already at the right place)
    #[cfg(all(M3_TARGET = "gem5", M3_ROTS = "1"))]
    {
        let rot_env: &env::BootEnv = unsafe { &*(cfg::ENV_START_ROT.as_ptr()) };
        let rots_env: &mut env::BootEnv = unsafe { &mut *(cfg::ENV_START.as_mut_ptr()) };
        *rots_env = *rot_env;
    }

    // init our own environment; at this point we can still access app_env, because it is mapped by
    // the gem5 loader for us. afterwards, our address space does not contain that anymore.

    {
        let mut env = TM_ENV.borrow_mut();
        env.tile_id = app_env().boot.tile_id;
        env.org_tile_desc = app_env().boot.tile_desc();
        if env.org_tile_desc.has_virtmem() {
            let (_pmp_tile, _pmp_off, pmp_size, _pmp_perm) = TCU::unpack_mem_ep(0).unwrap();
            env.tile_desc = kif::TileDesc::new_with_attr(
                env.org_tile_desc.tile_type(),
                env.org_tile_desc.isa(),
                pmp_size as usize,
                env.org_tile_desc.attr() | TileAttr::IMEM,
            );
        }
        else {
            env.tile_desc = env.org_tile_desc;
        }

        env.platform = app_env().boot.platform;
    }

    unsafe {
        __m3_init_libc(0, ptr::null(), ptr::null(), false);
    }

    io::init(
        tcu::TileId::new_from_raw(pex_env().tile_id as u16),
        "unimux",
    );

    mux::init(crate::pex_env());
    hdl::init();

    sidecalls::basic_handlers_init();

    // check once in case we've already received a sidecall
    sidecalls::check();
    // wait for sidecalls from the kernel until the user activity was started
    loop {
        if hdl::user_ready_or_sleep() {
            break;
        }
    }

    // note that we only get here in no-preempt mode; in preempt mode we will directly return to
    // the application from the interrupt we received due to the start sidecall
    hdl::run_to_completion();
}
