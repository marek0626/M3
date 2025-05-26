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

use core::cmp::min;
use core::mem::size_of_val;

use base::elf::{ElfHeaderCommon, PHType};
use base::env::BootEnv;
use base::errors::Error;
use base::io::log::LogColor;
use base::io::{log, LogFlags, Read};
use base::kif::Perm;
use base::mem::{GlobOff, VirtAddr};
use base::tcu::TCU;
use base::util::math::round_up;
use base::vec::Vec;
use base::{env, format, log, mem, tcu, util};
use rot::{self, CtxData, RosaCtx};

fn write_args<S, I>(args: I, env_off: &mut GlobOff) -> (VirtAddr, usize)
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let (arg_buf, arg_ptrs, _) = env::collect_args(args, rot::MEM_ENV_START + *env_off);
    TCU::write_slice(crate::ENV_EP, &arg_buf[..], *env_off)
        .expect("Failed to write arguments to kernel tile");
    *env_off = round_up(
        *env_off + mem::size_of_val(&arg_buf[..]) as GlobOff,
        mem::size_of::<VirtAddr>() as GlobOff,
    );
    TCU::write_slice(crate::ENV_EP, &arg_ptrs[..], *env_off)
        .expect("Failed to write argument pointers to kernel tile");
    let argp = rot::MEM_ENV_START + *env_off;
    *env_off += mem::size_of_val(&arg_ptrs[..]) as GlobOff;
    (argp, arg_ptrs.len() - 1)
}

struct TCUReader {
    off: GlobOff,
}

impl TCUReader {
    fn seek(&mut self, pos: GlobOff) {
        self.off = pos;
    }
}

impl Read for TCUReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        TCU::read_slice(crate::COPY_EP, buf, self.off)?;
        self.off += buf.len() as GlobOff;
        Ok(buf.len())
    }
}

fn load_kernel_elf() {
    let mut rd = TCUReader { off: 0 };

    log!(LogFlags::RoTBoot, "Loading kernel");
    let hdr: ElfHeaderCommon =
        TCU::read_obj(crate::COPY_EP, 0).expect("Failed to read base kernel ELF header");
    log!(LogFlags::RoTDbg, "{:x?}", hdr);
    assert_eq!(&hdr.ident.magic[..4], b"\x7FELF", "Invalid ELF magic");

    let hdr = hdr
        .load_hdr(&mut rd)
        .expect("Failed to actual kernel ELF header");

    // SAFETY: COPY_BUF is only used in the (single-threaded) main boot path
    let copy_buf = unsafe { crate::COPY_BUF.get_mut() };
    let mut ph_off = hdr.ph_off();
    for _ in 0..hdr.ph_num() {
        // load program header
        rd.seek(ph_off as GlobOff);
        let phdr = hdr
            .load_ph(&mut rd)
            .expect("Failed to read ELF program header");
        ph_off += size_of_val(&*phdr);
        log!(LogFlags::RoTDbg, "{:x?}", phdr.as_ref());

        if phdr.ty() != PHType::Load as u32 || phdr.mem_size() == 0 {
            continue;
        }
        assert!(phdr.mem_size() >= phdr.file_size());

        let mut size = phdr.mem_size();
        let mut off = (phdr.phys_addr() - rot::MEM_OFFSET) as GlobOff;
        if phdr.file_size() > 0 {
            let mut copy = min(size, phdr.file_size());
            size -= copy;

            let mut elf_off = phdr.offset() as GlobOff;
            while copy > 0 {
                let len = min(copy, copy_buf.len());
                TCU::read(crate::COPY_EP, copy_buf.as_mut_ptr(), len, elf_off)
                    .expect("Failed to read ELF segment data from memory");
                TCU::write(crate::MEM_EP, copy_buf.as_ptr(), len, off)
                    .expect("Failed to write ELF segment data to kernel tile");
                elf_off += len as GlobOff;
                off += len as GlobOff;
                copy -= len;
            }
        }

        // BSS
        crate::clear_mem(off, size).expect("Failed to write BSS to kernel tile");
    }

    // for RISCV32 the execution starts at 0 and we need to jump to the actual entrypoint
    #[cfg(target_arch = "riscv32")]
    {
        let trampoline: [u32; 2] = [
            0x0001_22b7, // lui t0, 0x12 = 0x12000
            0x0000_8282, // jr  t0
        ];
        TCU::write_slice(crate::MEM_EP, &trampoline, rot::MEM_OFFSET as GlobOff)
            .expect("Failed to write kernel trampoline");
    }
}

fn load_kernel_env(ctx: &crate::RosaPrivateLayerCtx, cfg: &rot::RosaLayerCfg) {
    // Copy kernel arguments and environment variables
    let mut env_off = mem::size_of::<BootEnv>() as GlobOff;
    let kernel_cmdline = util::cstr_slice_to_str(&cfg.data.kernel_cmdline);

    // append "--root <root-tile>" to the arguments
    let mut kargs = kernel_cmdline.split(' ').collect::<Vec<_>>();
    kargs.push("--root");
    let root_tile_arg = format!("{}", ctx.data.root_tile.id().raw());
    kargs.push(&root_tile_arg);

    let (argv, argc) = write_args(kargs.iter(), &mut env_off);
    let (envp, _) = write_args(env::Vars::default(), &mut env_off);

    let env = BootEnv {
        platform: env::boot().platform,
        tile_id: ctx.data.kernel_tile.id().raw() as u64,
        tile_desc: ctx.data.kernel_tile_desc.value(),
        argc: argc as u64,
        argv: argv.as_raw(),
        envp: envp.as_raw(),
        kenv: ctx.data.kenv_addr.raw(),
        raw_tile_count: env::boot().raw_tile_count,
        raw_tile_ids: env::boot().raw_tile_ids,
    };
    log!(LogFlags::RoTDbg, "{:x?}", env);
    TCU::write_obj(crate::ENV_EP, &env, 0).expect("Failed to write BootEnv to kernel tile");
}

pub fn main() -> ! {
    log::init(env::boot().tile_id(), "rosa2", LogColor::Magenta);
    log!(LogFlags::RoTBoot, "Hello World");

    let ctx = unsafe { crate::RosaPrivateLayerCtx::get() };
    log!(LogFlags::RoTDbg, "{:#x?}", ctx);
    let cfg = unsafe { rot::RosaLayerCfg::get() };

    load_kernel_elf();
    load_kernel_env(ctx, cfg);

    log!(LogFlags::RoTDbg, "loading rots");
    // Load ROTS
    let _ = unsafe { rot::load_bin(rot::ROSA_ROTS_NEXT_ADDR, &cfg.data.next_layer) };
    // Fixup context
    ctx.entry_addr = rot::ROSA_NEXT_ADDR; // NMG This is where we load RoTS
    ctx.magic = RosaCtx::MAGIC;

    // invalidate all no-longer needed EPs
    ctx.data.our_tile.invalidate_ep(rot::FLASH_EP).unwrap();
    ctx.data.our_tile.invalidate_ep(crate::MEM_EP).unwrap();
    ctx.data.our_tile.invalidate_ep(crate::COPY_EP).unwrap();
    ctx.data.our_tile.invalidate_ep(crate::ENV_EP).unwrap();
    ctx.data.our_tile.invalidate_ep(crate::SELF_EP).unwrap();

    log!(LogFlags::RoTBoot, "Resetting kernel tile");
    ctx.data
        .kernel_tile
        .ext_cmd(tcu::TCU::build_ext_cmd(tcu::ExtCmdOpCode::Reset, 1))
        .expect("Failed to reset kernel tile");

    // reduce our TCU-MMIO-area permission to read-only
    ctx.data.our_tile.init(Perm::R);
    ctx.data.kernel_tile.init(Perm::R);

    log!(LogFlags::RoTDbg, "switch to rots");
    let next_ctx = rot::LayerCtx::new(rot::ROSA_ROTS_NEXT_ADDR, rot::RotsCtx {
        derived_private_key: ctx.data.next.derived_private_key.clone(),
        kmac_cdi: ctx.data.next.kmac_cdi.clone(),
        occupied_eps: ctx.data.next.occupied_eps,
    });
    // NMG Switch directly to RoTS instead of waiting for the kernel to reset/wake us.
    unsafe { next_ctx.switch() }
}
