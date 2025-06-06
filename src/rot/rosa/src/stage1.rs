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

use base::col::{BTreeMap, BTreeMapEntry, Vec};
use base::io::log::LogColor;
use base::io::{log, LogFlags};
use base::kif::boot::{Info, Mem, Mod};
use base::kif::{tilemux, Perm, TileAttr, TileType};
use base::mem::{GlobAddr, GlobOff};
use base::tcu::{ActId, TCU};
use base::util::math::round_up;
use base::{cfg, env, log, mem, tcu, util};
use rot::cert::{HashBuf, M3RawCertificate};
use rot::ed25519::{Signer, SigningKey};
use rot::{Hex, Secret};

use crate::idxtile::{self, IndexedTile};
use crate::{config_local_ep, EPS_PER_PAGE, EP_REGS_SIZE};

/// Helper macro to find the best position in an iterator that satisfies a condition.
/// The base condition must always be satisfied, the preferred conditions are tried
/// one by one until one is satisfied or none are left.
///
/// The current implementation is not very efficient, it iterates several times
/// checking the base condition over and over again.
macro_rules! find_best_position {
    ($iter:expr, |($idx:ident,$name:ident)| $base_cond:expr) => {
        $iter.enumerate().position(|($idx, $name)| $base_cond)
    };
    ($iter:expr, |($idx:ident,$name:ident)| $base_cond:expr,
     try => $prefer_cond:expr $(, try => $cond_tail:expr)* $(,)?) => {
        $iter.enumerate().position(|($idx, $name)| $base_cond && $prefer_cond)
            .or_else(|| find_best_position!($iter, |($idx, $name)| $base_cond $(, try => $cond_tail)*))
    };
}

fn determine_mem_tile(m3: &rot::cert::M3Payload<'_>) -> IndexedTile {
    // We just use the first mem tile for now and assume it has sufficient space
    let idx = m3
        .tiles
        .iter()
        .position(|t| t.tile_type() == TileType::Mem)
        .expect("Failed to find mem tile");
    pick_tile(m3, idx, "memory")
}

fn determine_kernel_tile(m3: &rot::cert::M3Payload<'_>) -> IndexedTile {
    let idx = {
        find_best_position!(
            m3.tiles.iter(),
            |(_idx, desc)| desc.is_programmable() && !desc.attr().contains(TileAttr::ROT),
            try => desc.has_virtmem() && desc.attr().contains(TileAttr::EFFI),
            try => desc.has_virtmem(),
            try => desc.attr().contains(TileAttr::EFFI),
        )
        .expect("No suitable tile found for kernel")
    };
    pick_tile(m3, idx, "kernel")
}

fn determine_root_tile(m3: &rot::cert::M3Payload<'_>, ktile: IndexedTile) -> IndexedTile {
    let idx = {
        find_best_position!(
            m3.tiles.iter(),
            |(idx, desc)| idx != ktile.index() && desc.is_programmable() && !desc.attr().contains(TileAttr::ROT),
            try => desc.has_virtmem(),
        )
        .expect("No suitable tile found for root")
    };
    pick_tile(m3, idx, "root")
}

fn determine_our_tile(m3: &rot::cert::M3Payload<'_>) -> IndexedTile {
    let idx = {
        find_best_position!(m3.tiles.iter(), |(_idx, desc)| desc.is_programmable()
            && desc.attr().contains(TileAttr::ROT))
        .expect("No suitable tile found for self")
    };
    pick_tile(m3, idx, "our")
}

fn pick_tile(m3: &rot::cert::M3Payload<'_>, idx: usize, name: &str) -> IndexedTile {
    let tile_raw = env::boot().raw_tile_ids[idx] as u16;
    let tile = TCU::nocid_to_tileid(tile_raw);
    log!(
        LogFlags::RoTBoot,
        "Found {} tile {} with desc: {:?}",
        name,
        tile,
        m3.tiles[idx]
    );
    IndexedTile::new(tile, idx)
}

fn load_modules<'p, 'c: 'p>(
    cfg: &'c rot::RosaLayerCfg,
    m3: &mut rot::cert::M3Payload<'p>,
    mem_tile: IndexedTile,
) -> (GlobOff, Vec<Mod>) {
    let mod_count = cfg.data.mod_count();
    let mut mods = Vec::with_capacity(mod_count + 1);
    let mut mem_offset = 0;

    // SAFETY: COPY_BUF is only used in the (single-threaded) main boot path
    let copy_buf = unsafe { crate::COPY_BUF.get_mut() };
    for m in &cfg.data.mods[0..mod_count] {
        let mname = m.name();
        let msize = m.size as usize;

        log!(
            LogFlags::RoTBoot,
            "Copying and hashing mod {} ({} KiB): {} -> {}",
            mname,
            msize / 1024,
            m.addr(),
            GlobAddr::new_with(mem_tile.id(), mem_offset)
        );

        // Make sure we don't read anything from inside the RoT tile
        assert_ne!(m.addr().tile(), env::boot().tile_id());

        let mut hash: Hex<HashBuf> = Hex::new_zeroed();
        config_local_ep(crate::COPY_EP, |regs| {
            TCU::config_mem(
                regs,
                rot::TCU_ACT_ID,
                m.addr().tile(),
                0,
                m.addr().offset(),
                msize,
                Perm::R,
            )
        });
        rot::copy_and_hash(
            rot::cert::HASH_TYPE,
            crate::COPY_EP,
            crate::MEM_EP,
            mem_offset,
            msize,
            &mut copy_buf[..],
            &mut hash[..],
        );
        log!(LogFlags::RoTBoot, "Hash: {}", hash);

        match m3.mods.entry(mname) {
            BTreeMapEntry::Vacant(e) => e.insert(hash),
            BTreeMapEntry::Occupied(entry) => {
                log!(
                    LogFlags::Error,
                    "Duplicate module {} with previous hash: {:?}. Skipping.",
                    mname,
                    entry.get()
                );
                continue;
            },
        };

        let new_addr = GlobAddr::new_with(mem_tile.id(), mem_offset);
        mods.push(Mod::new(new_addr, m.size, mname));
        mem_offset = round_up(mem_offset + msize as GlobOff, cfg::PAGE_SIZE as GlobOff);
    }

    (mem_offset, mods)
}

fn derive_cdi(ctx: &rot::BlauLayerCtx, m3: &rot::cert::M3Payload<'_>, next_ctx: &mut rot::RosaCtx) {
    let cdi_json = rot::json::to_string(&m3).expect("Failed to serialize config for CDI");
    let cdi_bytes = cdi_json.as_bytes();
    log!(
        LogFlags::RoTDbg,
        "CDI JSON ({} bytes): {}",
        cdi_json.as_bytes().len(),
        cdi_json,
    );
    rot::derive_cdi(&ctx.data.kmac_cdi, cdi_bytes, &mut next_ctx.kmac_cdi);
}

fn derive_public_key(next_ctx: &mut rot::RosaCtx) -> Hex<[u8; 32]> {
    rot::derive_key(
        &next_ctx.kmac_cdi,
        "ED25519",
        &[],
        &mut next_ctx.derived_private_key.secret[..],
    );
    if !rot::QUICK_BOOT {
        let next_sig_key = SigningKey::from_bytes(&next_ctx.derived_private_key.secret);
        log!(LogFlags::RoTDbg, "Derived next layer {:?}", next_sig_key);
        Hex(next_sig_key.verifying_key().to_bytes())
    }
    else {
        Hex::new_zeroed()
    }
}

fn create_signature(ctx: rot::BlauLayerCtx, m3: &rot::cert::M3Payload<'_>, dest: GlobAddr) -> Mod {
    let sign_raw = rot::json::value::to_raw_value(&m3).unwrap();
    log!(
        LogFlags::RoTDbg,
        "JSON to be signed ({} bytes): {}",
        sign_raw.get().as_bytes().len(),
        sign_raw.get(),
    );

    let (sig_key_bytes, signature) = if !rot::QUICK_BOOT {
        let sig_key = SigningKey::from_bytes(&ctx.data.derived_private_key.secret);
        let signature = Hex(sig_key.sign(sign_raw.get().as_bytes()).to_bytes());
        log!(LogFlags::RoTDbg, "Signed: {}", signature);
        (Hex(sig_key.verifying_key().to_bytes()), signature)
    }
    else {
        (Hex::new_zeroed(), Hex::new_zeroed())
    };

    let cert = M3RawCertificate {
        payload: sign_raw,
        signature,
        pub_key: sig_key_bytes,
        parent: rot::cert::Certificate {
            payload: ctx.data.signed_payload,
            signature: ctx.data.signature,
            pub_key: ctx.data.signer_public_key,
            parent: (),
        },
    };
    let cert_json = rot::json::to_string(&cert).expect("Failed to serialize certificate");
    let cert_json_size = cert_json.as_bytes().len();
    log!(
        LogFlags::RoTDbg,
        "rot-certificate.json ({} bytes): {}",
        cert_json_size,
        cert_json,
    );

    TCU::write_slice(crate::MEM_EP, cert_json.as_bytes(), dest.offset())
        .expect("Failed to write rot-certificate.json to DRAM");

    Mod::new(dest, cert_json_size as u64, "rot-certificate.json")
}

fn write_kenv(
    m3: &rot::cert::M3Payload<'_>,
    mods: &[Mod],
    mem_tile: IndexedTile,
    mem_size: usize,
    mem_offset: &mut GlobOff,
) -> (GlobOff, GlobOff, GlobOff) {
    const MEM_COUNT: usize = 1;
    let total_env_size = mem::size_of::<Info>()
        + mem::size_of_val(mods)
        + mem::size_of_val(&m3.tiles[..])
        + mem::size_of::<Mem>() * MEM_COUNT;

    let kenv_offset = *mem_offset;
    *mem_offset += total_env_size as GlobOff;
    let kenv_end = *mem_offset;
    *mem_offset = round_up(*mem_offset, cfg::PAGE_SIZE as GlobOff);
    let keps_offset = match env!("M3_TARGET") {
        "gem5" => *mem_offset,
        _ => 0,
    };
    *mem_offset += (m3.kernel.eps_num as usize * EP_REGS_SIZE) as GlobOff;
    let kernel_offset = *mem_offset;
    *mem_offset += m3.kernel.mem_size as GlobOff;

    let mems: [Mem; MEM_COUNT] = [Mem::new(
        GlobAddr::new_with(mem_tile.id(), *mem_offset),
        mem_size as GlobOff - *mem_offset,
        false,
    )];
    let info = Info {
        mod_count: mods.len() as u64,
        tile_count: m3.tiles.len() as u64,
        mem_count: mems.len() as u64,
        serv_count: 0,
    };
    log!(LogFlags::RoTDbg, "Boot {:?}", info);

    let mut off = kenv_offset;
    TCU::write_obj(crate::MEM_EP, &info, off).expect("Failed to write boot info");
    off += mem::size_of::<Info>() as GlobOff;
    TCU::write_slice(crate::MEM_EP, mods, off).expect("Failed to write mods");
    off += mem::size_of_val(mods) as GlobOff;
    TCU::write_slice(crate::MEM_EP, &m3.tiles[..], off).expect("Failed to write tiles");
    off += mem::size_of_val(&m3.tiles[..]) as GlobOff;
    TCU::write_slice(crate::MEM_EP, &mems[..], off).expect("Failed to write mems");
    off += mem::size_of_val(&mems[..]) as GlobOff;
    assert_eq!(off, kenv_end);

    (kenv_offset, keps_offset, kernel_offset)
}

#[cfg(M3_TARGET = "gem5")]
fn init_kernel_eps(
    m3: &rot::cert::M3Payload<'_>,
    mem_tile: IndexedTile,
    ktile: IndexedTile,
    keps_offset: GlobOff,
    kernel_offset: GlobOff,
) {
    crate::clear_mem(keps_offset, (kernel_offset - keps_offset) as usize)
        .expect("Failed to clear kernel endpoint region");

    const _: () = assert!(
        tcu::ExtReg::EpsAddr as u64 + 1 == tcu::ExtReg::EpsSize as u64,
        "EpsAddr and EpsSize must be consecutive registers (or code needs changes)"
    );

    let eps_addr =
        ((TCU::tileid_to_nocid(mem_tile.id()) as tcu::Reg) << 50) | keps_offset as tcu::Reg;
    ktile
        .write_tcu(
            &[eps_addr, kernel_offset - keps_offset],
            TCU::ext_reg_addr(tcu::ExtReg::EpsAddr).as_goff(),
        )
        .expect("Failed to configure endpoint memory region in kernel tile TCU");

    // Configure kernel's first PMP EP
    ktile.config_ep(0, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            mem_tile.id(),
            0,
            kernel_offset,
            m3.kernel.mem_size as usize,
            Perm::RWX,
        )
    });
}

#[allow(unused)]
fn prepare_for_rots(our_tile: IndexedTile, root_tile: IndexedTile) {
    // configure unimux's sidecall EP
    let desc = env::boot().tile_desc();
    let mut rbuf = desc.rbuf_mux_space().0.as_phys(desc);
    rbuf += 1 << cfg::KPEX_RBUF_ORD;
    config_local_ep(tcu::TMSIDE_REP, |regs| {
        TCU::config_recv(
            regs,
            tilemux::ACT_ID as ActId,
            rbuf,
            cfg::TMUP_RBUF_ORD,
            cfg::TMUP_RBUF_ORD,
            Some(tcu::TMSIDE_RPLEP),
        );
    });

    // configure exclusive region for the environment, accessible only from the root tile
    #[cfg(M3_TARGET = "gem5")]
    if let Some((cmd, arg1)) = TCU::build_exreg_cmd(
        our_tile.id(),
        root_tile.id(),
        0,
        0,
        cfg::ENV_START_DEF.as_goff() + (cfg::ENV_SIZE as GlobOff) / 2,
        (cfg::ENV_SIZE / 2) as GlobOff,
        Perm::W,
        true,
    ) {
        let arg_addr = TCU::ext_reg_addr(tcu::ExtReg::ExtArg1).as_goff();
        our_tile.write_tcu(&[arg1], arg_addr).unwrap();
        our_tile.ext_cmd(cmd).unwrap();
    }
}

fn lock_tile(our_tile: IndexedTile) {
    // get the address of the register
    let reg_addr = TCU::ext_reg_addr(tcu::ExtReg::Features).as_goff();
    // get features
    let mut features: u64 = our_tile
        .read_tcu_obj(reg_addr)
        .expect("Failed to read object");
    // set locked bit
    features |= tcu::FeatureFlags::LOCKED.bits();
    // write it
    our_tile
        .write_tcu(&[features], reg_addr)
        .expect("failed to write slice");
}

pub fn run() -> crate::RosaPrivateCtx {
    log::init(env::boot().tile_id(), "rosa", LogColor::BrightMagenta);
    log!(LogFlags::RoTBoot, "Hello World");

    let ctx = unsafe { rot::BlauLayerCtx::take() };
    let cfg = unsafe { rot::RosaLayerCfg::get() };

    log!(LogFlags::RoTBoot, "Scanning tiles");
    let tiles = env::boot().raw_tile_ids[0..env::boot().raw_tile_count as usize]
        .iter()
        .enumerate()
        .map(|(idx, id)| {
            // configure EP to access the remote TCU's MMIO region
            let tile = IndexedTile::new(TCU::nocid_to_tileid(*id as u16), idx);
            let perm = if tile.id() == env::boot().tile_id() {
                Perm::RW
            }
            else {
                Perm::R
            };
            tile.init(perm);

            // read out tile description
            tile.read_tcu_obj(TCU::ext_reg_addr(tcu::ExtReg::TileDesc).as_goff())
                .expect("Failed to read tile desc")
        })
        .collect();
    log!(LogFlags::RoTDbg, "Tiles: {:#?}", tiles);

    let mut m3 = rot::cert::M3Payload {
        tiles,
        kernel: rot::cert::M3KernelConfig {
            mem_size: cfg.data.kernel_mem_size,
            eps_num: cfg.data.kernel_ep_pages as u32 * EPS_PER_PAGE as u32,
            cmdline: util::cstr_slice_to_str(&cfg.data.kernel_cmdline),
        },
        mods: BTreeMap::new(),
        pub_key: Hex::new_zeroed(),
    };
    if env!("M3_TARGET") != "gem5" {
        m3.kernel.eps_num = 0; // hw23 does not have virteps
    }

    let mem_tile = determine_mem_tile(&m3);

    // Configure memory endpoint that spans the entire memory tile
    config_local_ep(crate::MEM_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            mem_tile.id(),
            0,
            0,
            m3.tiles[mem_tile.index()].mem_size(),
            Perm::W,
        )
    });

    // Load modules
    let (mut mem_offset, mut mods) = load_modules(cfg, &mut m3, mem_tile);
    log!(LogFlags::RoTDbg, "Loaded modules: {:#?}", mods);
    log!(LogFlags::RoTDbg, "Module hashes: {:#?}", m3.mods);

    // Prepare next context
    let mut next_ctx = rot::RosaCtx {
        kmac_cdi: Secret::new_zeroed(),
        derived_private_key: Secret::new_zeroed(),
        occupied_eps: (idxtile::TILE_TCU_EP_START, m3.tiles.len()),
    };
    derive_cdi(&ctx, &m3, &mut next_ctx);
    m3.pub_key = derive_public_key(&mut next_ctx);

    // create signature and add it as boot module
    let sig_addr = GlobAddr::new_with(mem_tile.id(), mem_offset);
    let sig_mod = create_signature(ctx, &m3, sig_addr);
    mods.push(sig_mod);
    mem_offset += sig_mod.size;
    mem_offset = round_up(mem_offset, cfg::PAGE_SIZE as GlobOff);

    // write kernel environment
    #[allow(unused)]
    let (kenv_offset, keps_offset, kernel_offset) = write_kenv(
        &m3,
        &mods[..],
        mem_tile,
        m3.tiles[mem_tile.index()].mem_size(),
        &mut mem_offset,
    );

    // Find kernel module and configure endpoint for loading
    let kmod = mods
        .iter()
        .find(|&m| m.name() == "kernel")
        .expect("Failed to find kernel mod");
    log!(LogFlags::RoTBoot, "Found kernel: {:?}", kmod);

    config_local_ep(crate::COPY_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            kmod.addr().tile(),
            0,
            kmod.addr().offset(),
            kmod.size as usize,
            Perm::R,
        )
    });

    let ktile = determine_kernel_tile(&m3);
    assert_ne!(ktile.id(), env::boot().tile_id());
    // we need write access to the kernel EPs
    ktile.init(Perm::RW);

    // Setup memory region for kernel endpoints
    #[cfg(M3_TARGET = "gem5")]
    init_kernel_eps(&m3, mem_tile, ktile, keps_offset, kernel_offset);

    let root_tile = determine_root_tile(&m3, ktile);
    let our_tile = determine_our_tile(&m3);

    // Configure endpoint used to load kernel ELF
    config_local_ep(crate::MEM_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            ktile.id(),
            0,
            rot::MEM_OFFSET as GlobOff,
            m3.kernel.mem_size as usize,
            Perm::W,
        )
    });
    // Configure endpoint used to load kernel environment
    config_local_ep(crate::ENV_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            ktile.id(),
            0,
            rot::MEM_ENV_START.as_goff(),
            cfg::ENV_SIZE,
            Perm::W,
        )
    });

    // prepare execution of rots/unimux and lock tile as we're about to start the kernel
    prepare_for_rots(our_tile, root_tile);
    lock_tile(our_tile);

    crate::RosaPrivateCtx {
        next: next_ctx,
        our_tile,
        kernel_tile: ktile,
        root_tile,
        kernel_tile_desc: m3.tiles[ktile.index()],
        kenv_addr: GlobAddr::new_with(mem_tile.id(), kenv_offset),
    }
}
