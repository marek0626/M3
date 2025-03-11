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

use m3::cap::Selector;
use m3::cell::RefCell;
use m3::cfg::{PAGE_BITS, PAGE_SIZE};
use m3::client::MapFlags;
use m3::com::MemGate;
use m3::errors::{Code, Error};
use m3::kif::Perm;
use m3::mem::{GlobOff, VirtAddr};
use m3::rc::Rc;
use m3::syscalls;
use m3::tiles::{ChildActivity, DefaultMapper, Mapper, Tile};
use m3::util::math;
use m3::vec;
use m3::vec::Vec;
use m3::vfs::{BufReader, File, FileRef};
use resmng::resources::{memory, Resources};

use crate::AddrSpace;

pub(crate) struct ChildMapper<'a> {
    aspace: &'a mut AddrSpace,
    has_virtmem: bool,
    act_sel: Selector,
    def_mapper: DefaultMapper,
    tee: bool,
    tile: Rc<Tile>,
    mem_pool: Rc<RefCell<memory::MemPool>>,
    res: &'a mut Resources,
    allocs: Vec<memory::Allocation>,
    buf: Vec<u8>,
}

impl<'a> ChildMapper<'a> {
    pub fn new(
        aspace: &'a mut AddrSpace,
        has_virtmem: bool,
        act_sel: Selector,
        tee: bool,
        tile: Rc<Tile>,
        mem_pool: Rc<RefCell<memory::MemPool>>,
        res: &'a mut Resources,
    ) -> Self {
        ChildMapper {
            aspace,
            has_virtmem,
            act_sel,
            def_mapper: DefaultMapper::new(has_virtmem),
            tee,
            tile,
            mem_pool,
            res,
            allocs: Vec::new(),
            buf: vec![0u8; 4096],
        }
    }

    fn map_mem(
        &mut self,
        virt: VirtAddr,
        size: usize,
        perm: Perm,
    ) -> Result<(Selector, GlobOff), Error> {
        let alloc = self
            .mem_pool
            .borrow_mut()
            .allocate(size as GlobOff)
            .map_err(|e| e.downcast::<Error>().unwrap())?;
        let msel = self.mem_pool.borrow().mem_cap(alloc.slice_id());

        syscalls::create_map(
            virt,
            self.act_sel,
            msel,
            (alloc.addr() >> PAGE_BITS) as Selector,
            (size >> PAGE_BITS) as Selector,
            perm,
        )?;

        self.allocs.push(alloc);
        Ok((msel, alloc.addr()))
    }
}

impl Mapper for ChildMapper<'_> {
    fn buffer(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.buf)
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
        let size = math::round_up(mem_size, PAGE_SIZE);
        if self.tee {
            // This really should be an assert() because a mapper should have a pager only if it has virtmem
            if !self.has_virtmem {
                return Err(Error::new(Code::NotSup));
            }
            // Acquire a pointer/selector to a region inside the target activity's memory space
            let (mgate, moff) = {
                let (msel, moff) = self.map_mem(virt, size, perm)?;
                (MemGate::new_bind(msel)?, moff)
            };

            // And copy the file contents (which may be just a single segment each invocation) to the memory selector
            self.init(&mgate, file, foff, moff, file_size, mem_size)
        }
        else if self.has_virtmem {
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
        let size = math::round_up(mem_size, PAGE_SIZE);
        if self.tee {
            // This really should be an assert() because a mapper should have a pager only if it has virtmem
            if !self.has_virtmem {
                return Err(Error::new(Code::NotSup));
            }
            // Acquire a pointer/selector to a region inside the target activity's memory space
            let (mgate, moff) = {
                let (msel, moff) = self.map_mem(virt, size, perm)?;
                (MemGate::new_bind(msel)?, moff)
            };

            self.clear(&mgate, moff, mem_size)
        }
        else if self.has_virtmem {
            self.aspace
                .map_anon_with(virt, size as GlobOff, perm, flags)
                .map(|_| ())
        }
        else {
            self.def_mapper
                .map_anon(act, virt, file_size, mem_size, perm, flags)
        }
    }

    fn finished(&mut self) -> Result<(), Error> {
        if self.tee {
            self.mem_pool
                .borrow_mut()
                .make_exclusive(self.res, &self.tile)
                .map_err(|e| e.downcast::<Error>().unwrap())?;

            self.tile.lock()?;
        }
        Ok(())
    }
}
