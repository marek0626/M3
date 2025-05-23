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

use crate::Error;
use base::col::{BTreeMap, BTreeMapEntry, Vec};
use base::io::log::LogColor;
use base::io::{log, LogFlags};
use base::kif::boot::{Info, Mem, Mod};
use base::kif::{tilemux, Perm, TileAttr, TileType};
use base::mem::{GlobAddr, GlobOff};
use base::tcu::{ActId, TileId, TCU};
use base::util::math::round_up;
use base::{cfg, env, log, mem, tcu, util};
use rot::cert::{HashBuf, M3RawCertificate};
use rot::ed25519::{Signer, SigningKey};
use rot::{Hex, Secret};

const EP_REGS_SIZE: usize = tcu::EP_REGS * mem::size_of::<tcu::Reg>();
const EPS_PER_PAGE: usize = cfg::PAGE_SIZE / EP_REGS_SIZE;

fn config_local_ep<CFG>(ep: tcu::EpId, cfg: CFG)
where
    CFG: FnOnce(&mut [tcu::Reg]),
{
    let mut regs = [0; tcu::EP_REGS];
    cfg(&mut regs);
    TCU::set_ep_regs(ep, &regs);
}

fn config_remote_ep<CFG>(rtcu_ep: tcu::EpId, ep: tcu::EpId, cfg: CFG)
where
    CFG: FnOnce(&mut [tcu::Reg]),
{
    let mut regs = [0; tcu::EP_REGS];
    cfg(&mut regs);
    let off = (TCU::ep_regs_addr(ep) - tcu::MMIO_ADDR).as_goff();
    TCU::write_slice(rtcu_ep, &regs[..], off).expect("Failed to configure remote TCU endpoint");
}

fn config_local_ep_remote_tcu(tile: TileId, perm: Perm) {
    config_local_ep(crate::TILE_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            tile,
            0,
            tcu::MMIO_ADDR.as_goff(),
            // We never configure more than one remote EP at the moment
            tcu::MMIO_SIZE + tcu::MMIO_PRIV_SIZE + 1 * EP_REGS_SIZE,
            perm,
        );
    });
}

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

fn determine_mem_tile(m3: &rot::cert::M3Payload<'_>) -> (TileId, usize) {
    // We just use the first mem tile for now and assume it has sufficient space
    let idx = m3
        .tiles
        .iter()
        .position(|t| t.tile_type() == TileType::Mem)
        .expect("Failed to find mem tile");
    pick_tile(m3, idx, "memory")
}

fn determine_kernel_tile(m3: &rot::cert::M3Payload<'_>) -> (TileId, usize) {
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

fn determine_root_tile(m3: &rot::cert::M3Payload<'_>, ktile_idx: usize) -> (TileId, usize) {
    let idx = {
        find_best_position!(
            m3.tiles.iter(),
            |(idx, desc)| idx != ktile_idx && desc.is_programmable() && !desc.attr().contains(TileAttr::ROT),
            try => desc.has_virtmem(),
        )
        .expect("No suitable tile found for root")
    };
    pick_tile(m3, idx, "root")
}

fn determine_our_tile(m3: &rot::cert::M3Payload<'_>) -> (TileId, usize) {
    let idx = {
        find_best_position!(m3.tiles.iter(), |(_idx, desc)| desc.is_programmable()
            && desc.attr().contains(TileAttr::ROT))
        .expect("No suitable tile found for self")
    };
    pick_tile(m3, idx, "our")
}

fn pick_tile(m3: &rot::cert::M3Payload<'_>, idx: usize, name: &str) -> (TileId, usize) {
    let tile_raw = env::boot().raw_tile_ids[idx] as u16;
    let tile = TCU::nocid_to_tileid(tile_raw);
    log!(
        LogFlags::RoTBoot,
        "Found {} tile {} with desc: {:?}",
        name,
        tile,
        m3.tiles[idx]
    );
    (tile, idx)
}

fn load_modules<'p, 'c: 'p>(
    cfg: &'c rot::RosaLayerCfg,
    m3: &mut rot::cert::M3Payload<'p>,
    mem_tile: TileId,
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
            GlobAddr::new_with(mem_tile, mem_offset)
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

        let new_addr = GlobAddr::new_with(mem_tile, mem_offset);
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
    mem_tile: TileId,
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
    #[cfg(not(feature = "hw23"))]
    let keps_offset = *mem_offset;
    *mem_offset += (m3.kernel.eps_num as usize * EP_REGS_SIZE) as GlobOff;
    let kernel_offset = *mem_offset;
    *mem_offset += m3.kernel.mem_size as GlobOff;

    let mems: [Mem; MEM_COUNT] = [Mem::new(
        GlobAddr::new_with(mem_tile, *mem_offset),
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

#[allow(unused)]
fn init_kernel_eps(
    m3: &rot::cert::M3Payload<'_>,
    mem_tile: TileId,
    keps_offset: GlobOff,
    kernel_offset: GlobOff,
) {
    crate::clear_mem(keps_offset, (kernel_offset - keps_offset) as usize)
        .expect("Failed to clear kernel endpoint region");

    const _: () = assert!(
        tcu::ExtReg::EpsAddr as u64 + 1 == tcu::ExtReg::EpsSize as u64,
        "EpsAddr and EpsSize must be consecutive registers (or code needs changes)"
    );

    let eps_addr = ((TCU::tileid_to_nocid(mem_tile) as tcu::Reg) << 50) | keps_offset as tcu::Reg;
    TCU::write_slice(
        crate::TILE_EP,
        &[eps_addr, kernel_offset - keps_offset],
        (TCU::ext_reg_addr(tcu::ExtReg::EpsAddr) - tcu::MMIO_ADDR).as_goff(),
    )
    .expect("Failed to configure endpoint memory region in kernel tile TCU");

    // Configure kernel memory endpoint
    config_remote_ep(crate::TILE_EP, 0, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            mem_tile,
            0,
            kernel_offset,
            m3.kernel.mem_size as usize,
            Perm::RWX,
        )
    });
}

#[allow(unused)]
fn prepare_for_rots(our_tile: TileId, root_tile: TileId) {
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
    #[cfg(feature = "gem5")]
    if let Some((cmd, arg1)) = TCU::build_exreg_cmd(
        our_tile,
        root_tile,
        0,
        0,
        cfg::ENV_START_DEF.as_goff() + (cfg::ENV_SIZE as GlobOff) / 2,
        (cfg::ENV_SIZE / 2) as GlobOff,
        Perm::W,
        true,
    ) {
        let arg_addr = TCU::ext_reg_addr(tcu::ExtReg::ExtArg1).as_goff();
        mmio_write_slice(&[arg1], arg_addr).unwrap();
        do_ext_cmd(cmd).unwrap();
    }
}

fn lock_tile() {
    // get the address of the register
    let reg_addr = TCU::ext_reg_addr(tcu::ExtReg::Features).as_goff();
    // get features
    let mut features: u64 = mmio_read_obj(reg_addr).expect("Failed to read object");
    // set locked bit
    features |= tcu::FeatureFlags::LOCKED.bits();
    // write it
    mmio_write_slice(&[features], reg_addr).expect("failed to write slice");
}

pub fn main() -> ! {
    log::init(env::boot().tile_id(), "rosa", LogColor::BrightMagenta);
    log!(LogFlags::RoTBoot, "Hello World");

    let ctx = unsafe { rot::BlauLayerCtx::take() };
    let cfg = unsafe { rot::RosaLayerCfg::get() };

    log!(LogFlags::RoTBoot, "Scanning tiles");
    let tiles = env::boot().raw_tile_ids[0..env::boot().raw_tile_count as usize]
        .iter()
        .map(|id| {
            let tile_id = TCU::nocid_to_tileid(*id as u16);
            config_local_ep_remote_tcu(tile_id, Perm::R);
            TCU::read_obj(
                crate::TILE_EP,
                (TCU::ext_reg_addr(tcu::ExtReg::TileDesc) - tcu::MMIO_ADDR).as_goff(),
            )
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
    if cfg!(feature = "hw23") {
        m3.kernel.eps_num = 0; // hw23 does not have virteps
    }

    let (mem_tile, mem_tile_idx) = determine_mem_tile(&m3);

    // Configure memory endpoint that spans the entire memory tile
    config_local_ep(crate::MEM_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            mem_tile,
            0,
            0,
            m3.tiles[mem_tile_idx].mem_size(),
            Perm::W,
        )
    });

    // Load modules
    let (mut mem_offset, mut mods) = load_modules(&cfg, &mut m3, mem_tile);
    log!(LogFlags::RoTDbg, "Loaded modules: {:#?}", mods);
    log!(LogFlags::RoTDbg, "Module hashes: {:#?}", m3.mods);

    // Prepare next context
    let mut next_ctx = rot::RosaCtx {
        kmac_cdi: Secret::new_zeroed(),
        derived_private_key: Secret::new_zeroed(),
    };
    derive_cdi(&ctx, &m3, &mut next_ctx);
    m3.pub_key = derive_public_key(&mut next_ctx);

    // create signature and add it as boot module
    let sig_addr = GlobAddr::new_with(mem_tile, mem_offset);
    let sig_mod = create_signature(ctx, &m3, sig_addr);
    mods.push(sig_mod);
    mem_offset += sig_mod.size;
    mem_offset = round_up(mem_offset, cfg::PAGE_SIZE as GlobOff);

    // write kernel environment
    let (kenv_offset, keps_offset, kernel_offset) = write_kenv(
        &m3,
        &mods[..],
        mem_tile,
        m3.tiles[mem_tile_idx].mem_size(),
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

    let (ktile, ktile_idx) = determine_kernel_tile(&m3);
    assert_ne!(ktile, env::boot().tile_id());

    // Configure endpoint to kernel TCU
    config_local_ep_remote_tcu(ktile, Perm::RW);

    // Setup memory region for kernel endpoints
    #[cfg(not(feature = "hw23"))]
    init_kernel_eps(&m3, mem_tile, keps_offset, kernel_offset);

    let (root_tile, _root_tile_idx) = determine_root_tile(&m3, ktile_idx);
    let (our_tile, _our_tile_idx) = determine_our_tile(&m3);

    // Configure endpoint used to load kernel ELF
    config_local_ep(crate::MEM_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            ktile,
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
            ktile,
            0,
            rot::MEM_ENV_START.as_goff(),
            cfg::ENV_SIZE,
            Perm::W,
        )
    });
    // setup EP for our own TCU MMIO region
    config_local_ep(crate::SELF_EP, |regs| {
        TCU::config_mem(
            regs,
            rot::TCU_ACT_ID,
            our_tile,
            0,
            tcu::MMIO_ADDR.as_local() as GlobOff,
            tcu::MMIO_SIZE,
            Perm::W | Perm::R,
        )
    });

    // prepare execution of rots/unimux and lock tile as we're about to start the kernel
    prepare_for_rots(our_tile, root_tile);
    lock_tile();

    // Continue loading in second stage after clearing secrets
    let next_ctx = rot::LayerCtx::new(rot::ROSA_ADDR, crate::RosaPrivateCtx {
        next: next_ctx,
        kernel_tile_id: ktile.raw() as u64,
        kernel_tile_desc: m3.tiles[ktile_idx].value(),
        kenv_addr: GlobAddr::new_with(mem_tile, kenv_offset),
        root_tile_id: root_tile.raw() as u64,
    });
    unsafe { next_ctx.switch() }
}

fn mmio_write_slice<T>(sl: &[T], addr: GlobOff) -> Result<(), Error> {
    let sl_addr = sl.as_ptr() as *const u8;

    mmio_write_mem(sl_addr, mem::size_of_val(sl), addr)
}

fn mmio_write_mem(data: *const u8, size: usize, addr: GlobOff) -> Result<(), Error> {
    log!(LogFlags::RoTDbg, "writing {} bytes to {:#x}", size, addr);
    TCU::write(
        crate::SELF_EP,
        data,
        size,
        addr - tcu::MMIO_ADDR.as_local() as GlobOff,
    )
}

fn mmio_read_obj<T: Default>(addr: GlobOff) -> Result<T, Error> {
    let mut obj: T = T::default();
    let obj_addr = &mut obj as *mut T as *mut u8;
    mmio_read_mem(obj_addr, mem::size_of::<T>(), addr)?;
    Ok(obj)
}

fn mmio_read_mem(data: *mut u8, size: usize, addr: GlobOff) -> Result<(), Error> {
    log!(LogFlags::RoTDbg, "reading {} bytes from {:#x}", size, addr);
    TCU::read(
        crate::SELF_EP,
        data,
        size,
        addr - tcu::MMIO_ADDR.as_local() as GlobOff,
    )
}

#[cfg(feature = "gem5")]
fn do_ext_cmd(cmd: tcu::Reg) -> Result<tcu::Reg, Error> {
    let addr = TCU::ext_reg_addr(tcu::ExtReg::ExtCmd).as_goff();
    mmio_write_slice(&[cmd], addr)?;
    wait_ext_cmd()
}

#[cfg(feature = "gem5")]
fn wait_ext_cmd() -> Result<tcu::Reg, Error> {
    use base::errors::Code;

    let addr = TCU::ext_reg_addr(tcu::ExtReg::ExtCmd).as_goff();

    let res = loop {
        let res: tcu::Reg = mmio_read_obj(addr)?;
        let idle_code: tcu::Reg = tcu::ExtCmdOpCode::Idle.into();
        if (res & 0xF) == idle_code {
            break res;
        }
    };

    match Code::try_from(((res >> 4) & 0x3F) as u32).unwrap() {
        Code::Success => Ok(res >> 10),
        e => Err(Error::new(e)),
    }
}
