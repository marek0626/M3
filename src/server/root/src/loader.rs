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

use anyhow::anyhow;

use core::any::Any;
use core::cmp;
use core::fmt;

use m3::cap::Selector;
use m3::cell::RefCell;
use m3::cfg::{PAGE_BITS, PAGE_SIZE};
use m3::client::{HashInput, HashOutput, MapFlags, Pager};
use m3::col::Vec;
use m3::com::MemGate;
use m3::errors::{Code, Error};
use m3::io::{Read, Write};
use m3::kif::Perm;
use m3::mem::{GlobOff, VirtAddr};
use m3::rc::Rc;
use m3::syscalls;
use m3::tiles::ChildActivity;
use m3::tiles::Mapper;
use m3::tiles::Tile;
use m3::util::math;
use m3::vec;
use m3::vfs;

use resmng::resources::Resources;

use crate::memory;

pub struct BootFile {
    mgate: MemGate,
    size: usize,
    pos: usize,
}

impl BootFile {
    pub fn new(mgate: MemGate, size: usize) -> Self {
        BootFile {
            mgate,
            size,
            pos: 0,
        }
    }
}

impl vfs::File for BootFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    // not needed here
    fn fd(&self) -> vfs::Fd {
        0
    }

    fn set_fd(&mut self, _fd: vfs::Fd) {
    }

    fn stat(&self) -> Result<vfs::FileInfo, Error> {
        Ok(vfs::FileInfo {
            mode: vfs::FileMode::FILE_DEF,
            size: self.size,
            extents: 1,
            ..Default::default()
        })
    }

    fn file_type(&self) -> u8 {
        b'F'
    }
}

impl vfs::Seek for BootFile {
    fn seek(&mut self, off: usize, whence: vfs::SeekMode) -> Result<usize, Error> {
        match whence {
            vfs::SeekMode::Cur => self.pos += off,
            vfs::SeekMode::Set => self.pos = off,
            vfs::SeekMode::End => self.pos = self.size,
        }
        Ok(self.pos)
    }
}

impl Read for BootFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        if self.pos >= self.size {
            Ok(0)
        }
        else {
            let amount = cmp::min(buf.len(), self.size - self.pos);
            self.mgate.read(&mut buf[0..amount], self.pos as GlobOff)?;
            self.pos += amount;
            Ok(amount)
        }
    }
}

impl Write for BootFile {
    fn write(&mut self, _buf: &[u8]) -> Result<usize, Error> {
        Err(Error::new(Code::NotSup))
    }
}

impl vfs::Map for BootFile {
    fn map(
        &self,
        _pager: &Pager,
        _virt: VirtAddr,
        _off: usize,
        _len: usize,
        _prot: Perm,
        _flags: MapFlags,
    ) -> Result<(), Error> {
        // not used
        Ok(())
    }
}

impl HashInput for BootFile {
}
impl HashOutput for BootFile {
}

impl fmt::Debug for BootFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BootFile[sel={}, size={:#x}, pos={:#x}]",
            self.mgate.sel(),
            self.size,
            self.pos
        )
    }
}

pub struct BootMapper<'a> {
    act_sel: Selector,
    mem_sel: Selector,
    has_virtmem: bool,
    tee: bool,
    tile: Rc<Tile>,
    mem_pool: Rc<RefCell<memory::MemPool>>,
    res: &'a mut Resources,
    allocs: Vec<memory::Allocation>,
    buf: Vec<u8>,
}

impl<'a> BootMapper<'a> {
    pub fn new(
        act_sel: Selector,
        mem_sel: Selector,
        has_virtmem: bool,
        tee: bool,
        tile: Rc<Tile>,
        mem_pool: Rc<RefCell<memory::MemPool>>,
        res: &'a mut Resources,
    ) -> anyhow::Result<Self> {
        Ok(BootMapper {
            act_sel,
            mem_sel,
            has_virtmem,
            tee,
            tile,
            mem_pool,
            res,
            allocs: Vec::new(),
            buf: vec![0u8; 4096],
        })
    }

    pub fn fetch_allocs(self) -> Vec<memory::Allocation> {
        self.allocs
    }

    fn map_mem(
        &mut self,
        virt: VirtAddr,
        size: usize,
        perm: Perm,
    ) -> anyhow::Result<(Selector, GlobOff)> {
        let alloc = self.mem_pool.borrow_mut().allocate(size as GlobOff)?;
        let msel = self.mem_pool.borrow().mem_cap(alloc.slice_id());

        syscalls::create_map(
            virt,
            self.act_sel,
            msel,
            (alloc.addr() >> PAGE_BITS) as Selector,
            (size >> PAGE_BITS) as Selector,
            perm,
        )
        .map_err(|e| anyhow!(e).context("create map"))?;

        self.allocs.push(alloc);
        Ok((msel, alloc.addr()))
    }
}

impl Mapper for BootMapper<'_> {
    fn buffer(&mut self) -> Option<&mut [u8]> {
        Some(&mut self.buf)
    }

    fn map_file(
        &mut self,
        act: &ChildActivity,
        file: &mut vfs::BufReader<vfs::FileRef<dyn vfs::File>>,
        foff: usize,
        virt: VirtAddr,
        file_size: usize,
        mem_size: usize,
        perm: Perm,
        _flags: MapFlags,
    ) -> Result<(), Error> {
        let size = math::round_up(mem_size, PAGE_SIZE);

        // TEEs get a copy for every region
        if self.tee || perm.contains(Perm::W) {
            let (mgate, moff) = if self.has_virtmem {
                let (msel, moff) = self
                    .map_mem(virt, size, perm)
                    .map_err(|e| e.downcast::<Error>().unwrap())?;
                (MemGate::new_bind(msel)?, moff)
            }
            else {
                (act.get_mem(virt, size as GlobOff, Perm::RW)?, 0)
            };

            self.init(&mgate, file, foff, moff, file_size, mem_size)
        }
        else if self.has_virtmem {
            // map the memory of the boot module directly; therefore no initialization necessary
            syscalls::create_map(
                virt,
                self.act_sel,
                self.mem_sel,
                (foff >> PAGE_BITS) as Selector,
                (size >> PAGE_BITS) as Selector,
                perm,
            )
        }
        else {
            let mgate = act.get_mem(virt, size as GlobOff, Perm::RW)?;
            self.init(&mgate, file, foff, 0, file_size, mem_size)
        }
    }

    fn map_anon(
        &mut self,
        act: &ChildActivity,
        virt: VirtAddr,
        _file_size: usize,
        mem_size: usize,
        perm: Perm,
        flags: MapFlags,
    ) -> Result<(), Error> {
        let size = math::round_up(mem_size, PAGE_SIZE);
        if self.has_virtmem {
            let (msel, moff) = self
                .map_mem(virt, size, perm)
                .map_err(|e| e.downcast::<Error>().unwrap())?;

            if !flags.contains(MapFlags::UNINIT) {
                let mgate = MemGate::new_bind(msel)?;
                self.clear(&mgate, moff, size)?;
            }
            Ok(())
        }
        else {
            let mgate = act.get_mem(virt, size as GlobOff, Perm::RW)?;
            self.clear(&mgate, 0, mem_size)
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
