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

//! The mapper types that are used to init the memory of an activity.

use core::cmp;

use crate::cfg;
use crate::client::MapFlags;
use crate::col::Vec;
use crate::com::MemGate;
use crate::errors::{Code, Error};
use crate::io::Read;
use crate::kif;
use crate::mem::{GlobOff, VirtAddr};
use crate::tiles::ChildActivity;
use crate::util::math;
use crate::vec;
use crate::vfs::{BufReader, File, FileRef, Map, Seek, SeekMode};

/// The mapper trait is used to map the memory of an activity before running it.
pub trait Mapper {
    fn buffer(&mut self) -> Option<&mut [u8]> {
        None
    }

    /// Maps the given file to `virt`..`virt`+`len` with given permissions.
    #[allow(clippy::too_many_arguments)]
    fn map_file(
        &mut self,
        act: &ChildActivity,
        file: &mut BufReader<FileRef<dyn File>>,
        foff: usize,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: kif::Perm,
        flags: MapFlags,
    ) -> Result<(), Error>;

    /// Maps anonymous memory to `virt`..`virt`+`len` with given permissions.
    fn map_anon(
        &mut self,
        act: &ChildActivity,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: kif::Perm,
        flags: MapFlags,
    ) -> Result<(), Error>;

    fn init(
        &mut self,
        mem: &MemGate,
        file: &mut BufReader<FileRef<dyn File>>,
        file_offset: usize,
        mem_offset: GlobOff,
        file_size: usize,
        mem_size: usize,
    ) -> Result<(), Error> {
        let mut segoff = mem_offset;
        if file_size > 0 {
            file.seek(file_offset, SeekMode::Set)?;

            let buf = self.buffer().unwrap();
            let mut count = file_size;
            while count > 0 {
                let amount = cmp::min(count, buf.len());
                let amount = file.read(&mut buf[0..amount])?;

                mem.write_bytes(buf.as_mut_ptr(), amount, segoff)?;

                count -= amount;
                segoff += amount as GlobOff;
            }
        }

        self.clear(mem, segoff, mem_size - file_size)
    }

    fn clear(
        &mut self,
        mem: &MemGate,
        mut mem_offset: GlobOff,
        mut len: usize,
    ) -> Result<(), Error> {
        if len == 0 {
            return Ok(());
        }

        let buf = self.buffer().unwrap();
        for it in buf.iter_mut() {
            *it = 0;
        }

        while len > 0 {
            let amount = cmp::min(len, buf.len());
            mem.write_bytes(buf.as_mut_ptr(), amount, mem_offset)?;
            len -= amount;
            mem_offset += amount as GlobOff;
        }

        Ok(())
    }
}

/// The default implementation of the [`Mapper`] trait.
pub struct DefaultMapper {
    has_virtmem: bool,
    buf: Option<Vec<u8>>,
}

impl DefaultMapper {
    /// Creates a new `DefaultMapper`.
    pub fn new(has_virtmem: bool) -> Self {
        // without VM we are initializing everything eagerly using self.init()/self.clear(), which
        // requires a buffer.
        let buf = if !has_virtmem {
            Some(vec![0u8; 4096])
        }
        else {
            None
        };
        DefaultMapper { has_virtmem, buf }
    }
}

impl Mapper for DefaultMapper {
    fn buffer(&mut self) -> Option<&mut [u8]> {
        self.buf.as_deref_mut()
    }

    fn map_file(
        &mut self,
        act: &ChildActivity,
        file: &mut BufReader<FileRef<dyn File>>,
        foff: usize,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: kif::Perm,
        flags: MapFlags,
    ) -> Result<(), Error> {
        let size = math::round_up(mem_size, cfg::PAGE_SIZE);
        if let Some(pg) = act.pager() {
            file.get_ref().map(pg, virt, foff, size, perm, flags)
        }
        else if self.has_virtmem {
            // exec with VM, but without pager is not supported
            Err(Error::new(Code::NotSup))
        }
        else {
            let mgate = act.get_mem(virt, size as GlobOff, kif::Perm::RW)?;
            self.init(&mgate, file, foff, 0, file_size, mem_size)
        }
    }

    fn map_anon(
        &mut self,
        act: &ChildActivity,
        virt: VirtAddr,
        _file_size: usize,
        mem_size: usize,
        perm: kif::Perm,
        flags: MapFlags,
    ) -> Result<(), Error> {
        let size = math::round_up(mem_size, cfg::PAGE_SIZE);
        if let Some(pg) = act.pager() {
            pg.map_anon(virt, size, perm, flags).map(|_| ())
        }
        else if self.has_virtmem {
            // exec with VM, but without pager is not supported
            Err(Error::new(Code::NotSup))
        }
        else {
            let mgate = act.get_mem(virt, size as GlobOff, kif::Perm::RW)?;
            self.clear(&mgate, 0, mem_size)
        }
    }
}
