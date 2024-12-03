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

use anyhow::Context;

use core::mem::size_of_val;

use base::cfg::{MOD_HEAP_SIZE, PAGE_BITS, PAGE_MASK, PAGE_SIZE};
use base::env;
use base::errors::{Code, Error};
use base::io::{read_object, LogFlags, Read};
use base::kif::{self, PageFlags};
use base::log;
use base::mem::{size_of, GlobAddr, GlobOff, PhysAddr, VirtAddr};
use base::tcu;
use base::util::math;
use base::{elf, format};

use thread::{StrongRc, TempRc};

use crate::cap::{Capability, MapObject, SelRange};
use crate::ktcu;
use crate::mem;
use crate::tiles::{tilemng, Activity, TileMux};
use crate::{kerrno, kerror};

use crate::platform;

trait ELFLoader {
    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn load_segment_async(
        &mut self,
        virt: VirtAddr,
        phys: GlobAddr,
        size: usize,
        flags: PageFlags,
        map: bool,
    ) -> anyhow::Result<()>;

    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn zero_segment_async(
        &mut self,
        virt: VirtAddr,
        size: usize,
        flags: PageFlags,
    ) -> anyhow::Result<()>;

    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn map_heap_async(&mut self, _virt: VirtAddr) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn map_stack_async(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub fn init_activity_async(act: StrongRc<Activity>) -> anyhow::Result<i32> {
    let mut loader = ActivityELFLoader(act.clone());

    let root = act.is_root();
    let desc = platform::tile_desc(act.tile_id());

    // put mapping for env into cap table (so that we can access it in create_mgate later)
    let env_phys = if desc.has_virtmem() {
        let env_addr = TileMux::translate_async(
            tilemng::tilemux(act.tile_id()),
            act.id(),
            desc.env_space().0,
            kif::PageFlags::RW,
        )
        .with_context(|| "Retrieving global address of environment")?;

        let flags = PageFlags::from(kif::Perm::RW);
        loader.load_segment_async(desc.env_space().0, env_addr, PAGE_SIZE, flags, false)?;

        ktcu::glob_to_phys_remote(act.tile_id(), env_addr, flags).context(format!(
            "Translating {:?} to physical address for environment",
            env_addr
        ))?
    }
    else {
        desc.env_space().0.as_phys(desc)
    };

    if root {
        load_root_async(loader, env_phys).context("Loading root")?;
    }
    Ok(0)
}

pub fn load_mux_async(tile: tcu::TileId, mem: &mem::Allocation) -> anyhow::Result<()> {
    let desc = platform::tile_desc(tile);

    let app = get_mod("tilemux")
        .ok_or_else(|| kerrno(Code::NoSuchFile).context("No bootmodule 'tilemux'"))?;
    log!(
        LogFlags::KernActs,
        "Loading multiplexer '{}' onto {}",
        app.name(),
        tile
    );

    // load multiplexer into memory
    let mut loader = MetalELFLoader::new(mem.global(), desc.mem_offset() as GlobOff);
    load_mod_async(&mut loader, app).context("Loading TileMux")?;

    // write env vars
    let env_mem_off =
        mem.global().offset() + desc.env_space().0.as_goff() - desc.mem_offset() as GlobOff;
    let mut env_off = size_of::<env::BaseEnv>();
    let envp_addr = write_arguments(
        &env::vars_raw(),
        tile,
        mem.global().tile(),
        env_mem_off,
        &mut env_off,
    );

    // load environment into memory
    let env = env::BootEnv {
        platform: env::boot().platform,
        envp: envp_addr.as_raw(),
        tile_id: tile.raw() as u64,
        tile_desc: desc.value(),
        raw_tile_count: env::boot().raw_tile_count,
        raw_tile_ids: env::boot().raw_tile_ids,
        ..Default::default()
    };
    ktcu::write_slice(mem.global().tile(), env_mem_off, &[env]);

    Ok(())
}

fn load_root_async(mut loader: ActivityELFLoader, env_phys: PhysAddr) -> anyhow::Result<()> {
    let entry = {
        let app = get_mod("root")
            .ok_or_else(|| kerrno(Code::NoSuchFile).context("No bootmodule 'root'"))?;
        log!(LogFlags::KernActs, "Loading boot module '{}'", app.name());
        load_mod_async(&mut loader, app).context("Loading root")?
    };

    let act = &loader.0;
    let mut env_off = size_of::<env::BaseEnv>();
    let argv_addr = write_arguments(
        &["root"],
        act.tile_id(),
        act.tile_id(),
        env_phys.as_goff(),
        &mut env_off,
    );
    let envp_addr = write_arguments(
        &env::vars_raw(),
        act.tile_id(),
        act.tile_id(),
        env_phys.as_goff(),
        &mut env_off,
    );

    // write env to target tile
    let senv = env::BaseEnv {
        boot: env::BootEnv {
            platform: env::boot().platform,
            argc: 1,
            argv: argv_addr.as_raw(),
            envp: envp_addr.as_raw(),
            tile_id: act.tile_id().raw() as u64,
            tile_desc: act.tile_desc().value(),
            raw_tile_count: env::boot().raw_tile_count,
            raw_tile_ids: env::boot().raw_tile_ids,
            ..Default::default()
        },
        sp: act.tile_desc().stack_top().as_raw(),
        entry: entry.as_raw(),
        act_id: act.id() as u64,
        heap_size: MOD_HEAP_SIZE as u64,
        rmng_sel: kif::INVALID_SEL,
        first_sel: act.first_sel(),
        first_std_ep: act.eps_start() as u64,
        ..Default::default()
    };
    ktcu::write_slice(act.tile_id(), env_phys.as_goff(), &[senv]);

    Ok(())
}

fn get_mod(name: &str) -> Option<&kif::boot::Mod> {
    for m in platform::mods() {
        if let Some(bin_name) = m.name().split(' ').next() {
            if bin_name == name {
                return Some(m);
            }
        }
    }

    None
}

struct KernelBootMod<'a> {
    bm: &'a kif::boot::Mod,
    off: GlobOff,
}

impl KernelBootMod<'_> {
    fn seek(&mut self, pos: GlobOff) {
        self.off = pos;
    }
}

impl Read for KernelBootMod<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        if self.off + buf.len() as GlobOff > self.bm.size {
            return Err(Error::new(Code::InvalidElf));
        }

        let gaddr = GlobAddr::new(self.bm.addr);
        ktcu::read_slice(gaddr.tile(), gaddr.offset() + self.off, buf);
        self.off += buf.len() as GlobOff;

        Ok(buf.len())
    }
}

fn load_mod_async<L>(loader: &mut L, bm: &kif::boot::Mod) -> anyhow::Result<VirtAddr>
where
    L: ELFLoader,
{
    let mod_addr = GlobAddr::new(bm.addr);

    let mut kbm = KernelBootMod { bm, off: 0 };
    let hdr: elf::ElfHeaderCommon =
        read_object(&mut kbm).map_err(|e| kerror(e).context("Reading ELF header"))?;
    hdr.ident
        .check_magic()
        .map_err(|e| kerror(e).context("Invalid ELF magic"))?;

    kbm.seek(0);
    let hdr = hdr.load_hdr(&mut kbm).map_err(kerror)?;

    // copy load segments to destination tile
    let mut end = VirtAddr::default();
    let mut off = hdr.ph_off();
    for _ in 0..hdr.ph_num() {
        // load program header
        kbm.seek(off as GlobOff);
        let phdr = hdr
            .load_ph(&mut kbm)
            .map_err(|e| kerror(e).context("Loading PH"))?;
        off += size_of_val(&*phdr);

        // we're only interested in non-empty load segments
        if phdr.ty() != elf::PHType::Load.into() || phdr.mem_size() == 0 {
            continue;
        }

        let flags = PageFlags::from(kif::Perm::from(elf::PHFlags::from_bits_truncate(
            phdr.flags(),
        )));
        let offset = math::round_dn(phdr.offset(), PAGE_SIZE);
        let virt = VirtAddr::from(math::round_dn(phdr.virt_addr(), PAGE_SIZE));

        // bss?
        if phdr.file_size() == 0 {
            let size = math::round_up((phdr.virt_addr() & PAGE_MASK) + phdr.mem_size(), PAGE_SIZE);

            loader
                .zero_segment_async(virt, size, flags)
                .context(format!("Zero segment {}:{}", virt, size))?;
            end = virt + size;
        }
        else {
            assert!(phdr.mem_size() == phdr.file_size());
            let size = (phdr.offset() & PAGE_MASK) + phdr.file_size();
            loader
                .load_segment_async(virt, mod_addr + offset as GlobOff, size, flags, true)
                .context(format!("Load segment {}:{}", virt, size))?;
            end = virt + size;
        }
    }

    // map heap and stack
    let end = math::round_up(end, VirtAddr::from(PAGE_SIZE));
    loader.map_heap_async(end)?;
    loader.map_stack_async()?;

    Ok(VirtAddr::from(hdr.entry()))
}

struct MetalELFLoader {
    dst: GlobAddr,
    offset: GlobOff,
}

impl MetalELFLoader {
    fn new(dst: GlobAddr, offset: GlobOff) -> Self {
        Self { dst, offset }
    }
}

impl ELFLoader for MetalELFLoader {
    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn load_segment_async(
        &mut self,
        virt: VirtAddr,
        phys: GlobAddr,
        size: usize,
        _flags: PageFlags,
        _map: bool,
    ) -> anyhow::Result<()> {
        ktcu::copy(
            // destination
            self.dst.tile(),
            self.dst.offset() + virt.as_goff() - self.offset,
            // source
            phys.tile(),
            phys.offset(),
            size,
        )
    }

    #[cfg_attr(dylint_lib = "m3_lints", allow(unneeded_async))]
    fn zero_segment_async(
        &mut self,
        virt: VirtAddr,
        size: usize,
        _flags: PageFlags,
    ) -> anyhow::Result<()> {
        ktcu::clear(
            self.dst.tile(),
            self.dst.offset() + virt.as_goff() - self.offset,
            size,
        )
    }
}

struct ActivityELFLoader(StrongRc<Activity>);

impl ELFLoader for ActivityELFLoader {
    fn load_segment_async(
        &mut self,
        virt: VirtAddr,
        phys: GlobAddr,
        size: usize,
        flags: PageFlags,
        map: bool,
    ) -> anyhow::Result<()> {
        let tile_id = self.0.tile_id();

        if self.0.tile_desc().has_virtmem() {
            let dst_sel = (virt >> PAGE_BITS).as_raw() as kif::CapSel;
            let pages = math::round_up(size, PAGE_SIZE) >> PAGE_BITS;

            let phys_align = GlobAddr::new_with(phys.tile(), phys.offset() & !PAGE_MASK as GlobOff);
            let map_obj = MapObject::new(phys_align, flags);

            if map {
                MapObject::map_async(
                    TempRc::new(map_obj.clone()),
                    self.0.id(),
                    tile_id,
                    virt & VirtAddr::from(!PAGE_MASK),
                    phys_align,
                    pages,
                    flags,
                )?;
            }

            self.0.map_caps().borrow_mut().insert(Capability::new_range(
                SelRange::new_range(dst_sel as kif::CapSel, pages as kif::CapSel),
                map_obj,
            ))
        }
        else {
            MetalELFLoader::new(GlobAddr::new_with(tile_id, 0), 0)
                .load_segment_async(virt, phys, size, flags, map)
        }
    }

    fn zero_segment_async(
        &mut self,
        virt: VirtAddr,
        size: usize,
        flags: PageFlags,
    ) -> anyhow::Result<()> {
        let tile_id = self.0.tile_id();
        let tile_desc = self.0.tile_desc();

        let phys = if tile_desc.has_virtmem() {
            let mem = mem::borrow_mut()
                .allocate(mem::MemType::ROOT, size as GlobOff, PAGE_SIZE as GlobOff)
                .with_context(|| format!("Allocating {}b for segment {}", size, virt))?;
            self.load_segment_async(virt, mem.global(), size, flags, true)?;

            ktcu::glob_to_phys_remote(tile_id, mem.global(), flags)?
        }
        else {
            virt.as_phys(tile_desc)
        };

        ktcu::clear(tile_id, phys.as_goff(), size)
    }

    fn map_heap_async(&mut self, virt: VirtAddr) -> anyhow::Result<()> {
        let tile_desc = self.0.tile_desc();

        if tile_desc.has_virtmem() {
            let phys = mem::borrow_mut()
                .allocate(
                    mem::MemType::ROOT,
                    MOD_HEAP_SIZE as GlobOff,
                    PAGE_SIZE as GlobOff,
                )
                .with_context(|| format!("Allocating {}b for heap", MOD_HEAP_SIZE))?;
            self.load_segment_async(virt, phys.global(), MOD_HEAP_SIZE, PageFlags::RW, true)?;
        }
        Ok(())
    }

    fn map_stack_async(&mut self) -> anyhow::Result<()> {
        let tile_desc = self.0.tile_desc();

        if tile_desc.has_virtmem() {
            let (virt, size) = tile_desc.stack_space();
            let phys = mem::borrow_mut()
                .allocate(mem::MemType::ROOT, size as GlobOff, PAGE_SIZE as GlobOff)
                .with_context(|| format!("Allocating {}b for stack", size))?;
            self.load_segment_async(virt, phys.global(), size, PageFlags::RW, true)?;
        }
        Ok(())
    }
}

fn write_arguments<S>(
    args: &[S],
    dst_tile: tcu::TileId,
    mem_tile: tcu::TileId,
    env_mem_off: GlobOff,
    env_off: &mut usize,
) -> VirtAddr
where
    S: AsRef<str>,
{
    let env_start = platform::tile_desc(dst_tile).env_space().0;

    let (arg_buf, arg_ptr, arg_end) = env::collect_args(args, env_start + *env_off);

    // write actual arguments to memory
    ktcu::write_mem(
        mem_tile,
        env_mem_off + *env_off as GlobOff,
        arg_buf.as_ptr(),
        arg_buf.len(),
    );

    // write argument pointers to memory
    let arg_ptr_off = math::round_up(arg_end - env_start, VirtAddr::from(size_of::<u64>()));
    ktcu::write_mem(
        mem_tile,
        env_mem_off + arg_ptr_off.as_goff(),
        arg_ptr.as_ptr() as *const _,
        arg_ptr.len() * size_of::<u64>(),
    );

    *env_off = arg_ptr_off.as_local() + arg_ptr.len() * size_of::<u64>();
    env_start + arg_ptr_off
}
