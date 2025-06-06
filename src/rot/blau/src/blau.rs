/*
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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
#![no_main]

use core::arch::global_asm;

use base::io::log::LogColor;
use base::io::{log, LogFlags};
use base::{env, log, machine};
#[allow(unused_imports)]
use lang as _;
use rot::cert::{BinaryPayload, SignaturePayload};
use rot::ed25519::{SecretKey, Signer, SigningKey};
use rot::{Hex, Secret};

global_asm!(
    ".section .init.reset, \"ax\"",
    ".global _reset",
    "_reset:",
    "j      _start",
);

#[no_mangle]
pub extern "C" fn exit(_code: i32) -> ! {
    log!(LogFlags::Info, "Shutting down");
    machine::shutdown();
}

#[no_mangle]
pub extern "C" fn abort() {
    exit(1);
}

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    log::init(env::boot().tile_id(), "blau", LogColor::BrightBlue);
    log!(LogFlags::RoTBoot, "Hello World");

    let ctx = unsafe { rot::BromLayerCtx::take() };
    let cfg = unsafe { rot::BlauLayerCfg::get() };

    // Load binary for next layer and derive CDI
    let next = unsafe { rot::load_bin(rot::BLAU_NEXT_ADDR, &cfg.data.next_layer) };
    let mut next_cdi = Secret::new_zeroed();
    rot::derive_cdi(&ctx.data.kmac_cdi, next, &mut next_cdi);

    // Derive signing key used by next layer
    let mut next_seed: Secret<SecretKey> = Secret::new_zeroed();
    rot::derive_key(&next_cdi, "ED25519", &[], &mut next_seed.secret[..]);
    let next_sig_key_bytes = if !rot::QUICK_BOOT {
        let next_sig_key = SigningKey::from_bytes(&next_seed.secret);
        log!(LogFlags::RoTDbg, "Derived next layer {:?}", next_sig_key);
        Hex(next_sig_key.verifying_key().to_bytes())
    }
    else {
        Hex::new_zeroed()
    };

    // Prepare signature payload by hashing next layer again
    let mut payload = BinaryPayload {
        hash: Hex::new_zeroed(),
        pub_key: next_sig_key_bytes,
    };
    rot::hash(rot::cert::HASH_TYPE, next, &mut payload.hash[..]);
    log!(LogFlags::RoTBoot, "{:#?}", payload);

    // Derive own signing key
    let mut seed: Secret<SecretKey> = Secret::new_zeroed();
    rot::derive_key(&ctx.data.kmac_cdi, "ED25519", &[], &mut seed.secret[..]);

    // Create signature
    let (signature, sig_key_bytes) = if !rot::QUICK_BOOT {
        let sig_key = SigningKey::from_bytes(&seed.secret);
        log!(LogFlags::RoTDbg, "Derived own {:?}", sig_key);
        (
            Hex(sig_key.sign(payload.as_bytes()).to_bytes()),
            Hex(sig_key.verifying_key().to_bytes()),
        )
    }
    else {
        (Hex::new_zeroed(), Hex::new_zeroed())
    };
    log!(LogFlags::Info, "Verification key: {}", sig_key_bytes);
    log!(LogFlags::RoTDbg, "Signed: {}", signature);

    // Switch to next layer
    let next_ctx = rot::LayerCtx::new(rot::BLAU_NEXT_ADDR, rot::BlauCtx {
        kmac_cdi: next_cdi,
        derived_private_key: next_seed,
        signer_public_key: sig_key_bytes,
        signature,
        signed_payload: payload,
    });
    unsafe { next_ctx.switch() }
}
