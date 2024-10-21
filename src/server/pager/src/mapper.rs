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

use m3::cfg::PAGE_SIZE;
use m3::client::MapFlags;
use m3::errors::Error;
use m3::kif::Perm;
use m3::mem::{GlobOff, VirtAddr};
use m3::tiles::{ChildActivity, DefaultMapper, Mapper};
use m3::util::math;
use m3::vfs::{BufReader, File, FileRef};

use crate::AddrSpace;

pub(crate) struct ChildMapper<'a> {
    aspace: &'a mut AddrSpace,
    has_virtmem: bool,
    def_mapper: DefaultMapper,
}

impl<'a> ChildMapper<'a> {
    pub fn new(aspace: &'a mut AddrSpace, has_virtmem: bool) -> Self {
        ChildMapper {
            aspace,
            has_virtmem,
            def_mapper: DefaultMapper::new(has_virtmem),
        }
    }
}

impl<'a> Mapper for ChildMapper<'a> {
    fn buffer(&mut self) -> Option<&mut [u8]> {
        self.def_mapper.buffer()
    }

    fn map_file(
        &mut self,
        act: &ChildActivity,
        file: &mut BufReader<FileRef<dyn File>>,
        foff: usize,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: Perm,
        flags: MapFlags,
    ) -> Result<(), Error> {
        if self.has_virtmem {
            let size = math::round_up(mem_size, PAGE_SIZE);
            let sess = file.get_ref().session().unwrap();
            self.aspace
                .map_ds_with(virt, size as GlobOff, foff as GlobOff, perm, flags, sess)
                .map(|_| ())
        }
        else {
            self.def_mapper
                .map_file(act, file, foff, virt, file_size, mem_size, perm, flags)
        }
    }

    fn map_anon(
        &mut self,
        act: &ChildActivity,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: Perm,
        flags: MapFlags,
    ) -> Result<(), Error> {
        if self.has_virtmem {
            let size = math::round_up(mem_size, PAGE_SIZE);
            self.aspace
                .map_anon_with(virt, size as GlobOff, perm, flags)
                .map(|_| ())
        }
        else {
            self.def_mapper
                .map_anon(act, virt, file_size, mem_size, perm, flags)
        }
    }
}
