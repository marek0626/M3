/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
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

use core::mem::size_of_val;

use crate::cfg;
use crate::client::MapFlags;
use crate::elf;
use crate::errors::Error;
use crate::io::{read_object, LogFlags};
use crate::kif;
use crate::log;
use crate::mem::VirtAddr;
use crate::tiles::{ChildActivity, Mapper};
use crate::util::math;
use crate::vfs::{BufReader, File, FileRef, Seek, SeekMode};

pub(crate) fn load_program(
    act: &ChildActivity,
    mapper: &mut dyn Mapper,
    file: &mut BufReader<FileRef<dyn File>>,
) -> Result<VirtAddr, Error> {
    let hdr: elf::ElfHeaderCommon = read_object(file)?;
    hdr.ident.check_magic()?;

    file.seek(0, SeekMode::Set)?;
    let hdr = hdr.load_hdr(file)?;
    log!(LogFlags::LibLoader, "Found entrypoint {:#x}", hdr.entry());

    let heap_begin = load_segments(act, mapper, file, hdr.as_ref())?;
    create_heap(act, mapper, heap_begin)?;
    create_stack(act, mapper)?;

    Ok(VirtAddr::from(hdr.entry()))
}

fn create_stack(act: &ChildActivity, mapper: &mut dyn Mapper) -> Result<(), Error> {
    let (stack_addr, stack_size) = act.tile_desc().stack_space();
    log!(
        LogFlags::LibLoader,
        "Creating stack @ {} .. {} ({}b)",
        stack_addr,
        stack_addr + stack_size - 1,
        stack_size
    );
    mapper.map_anon(
        act,
        stack_addr,
        0,
        stack_size,
        kif::Perm::RW,
        MapFlags::PRIVATE | MapFlags::UNINIT,
    )
}

fn create_heap(act: &ChildActivity, mapper: &mut dyn Mapper, start: VirtAddr) -> Result<(), Error> {
    let (heap_size, flags) = if act.pager().is_some() {
        (cfg::APP_HEAP_SIZE, MapFlags::NOLPAGE)
    }
    else {
        (cfg::MOD_HEAP_SIZE, MapFlags::empty())
    };
    log!(
        LogFlags::LibLoader,
        "Creating heap @ {} with {}b",
        start,
        heap_size
    );
    mapper.map_anon(
        act,
        start,
        0,
        heap_size,
        kif::Perm::RW,
        MapFlags::PRIVATE | MapFlags::UNINIT | flags,
    )
}

fn load_segments(
    act: &ChildActivity,
    mapper: &mut dyn Mapper,
    file: &mut BufReader<FileRef<dyn File>>,
    hdr: &dyn elf::ElfHeader,
) -> Result<VirtAddr, Error> {
    let mut end = 0;
    let mut off = hdr.ph_off();
    for _ in 0..hdr.ph_num() {
        // load program header
        file.seek(off, SeekMode::Set)?;
        let phdr = hdr.load_ph(file)?;
        off += size_of_val(&*phdr);

        // we're only interested in non-empty load segments
        if phdr.ty() != elf::PHType::Load.into() || phdr.mem_size() == 0 {
            continue;
        }

        load_segment(act, mapper, file, phdr.as_ref())?;

        end = phdr.virt_addr() + phdr.mem_size();
    }

    Ok(VirtAddr::from(math::round_up(end, cfg::PAGE_SIZE)))
}

fn load_segment(
    act: &ChildActivity,
    mapper: &mut dyn Mapper,
    file: &mut BufReader<FileRef<dyn File>>,
    phdr: &dyn elf::ProgramHeader,
) -> Result<(), Error> {
    let prot = kif::Perm::from(elf::PHFlags::from_bits_truncate(phdr.flags()));
    let virt = VirtAddr::from(phdr.virt_addr());

    log!(
        LogFlags::LibLoader,
        "Load segment @ {} with {}b",
        virt,
        phdr.mem_size()
    );

    if phdr.mem_size() == phdr.file_size() {
        mapper.map_file(
            act,
            file,
            phdr.offset(),
            virt,
            phdr.file_size(),
            phdr.mem_size(),
            prot,
            MapFlags::PRIVATE,
        )
    }
    else {
        assert!(phdr.file_size() == 0);
        mapper.map_anon(
            act,
            virt,
            phdr.file_size(),
            phdr.mem_size(),
            prot,
            MapFlags::PRIVATE,
        )
    }
}
