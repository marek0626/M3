/*
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
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

use core::fmt;

use m3::cap::Selector;
use m3::cfg;
use m3::col::Vec;
use m3::com::MemCap;
use m3::errors::Code;
use m3::format;
use m3::io::LogFlags;
use m3::kif::Perm;
use m3::log;
use m3::mem::{GlobAddr, GlobOff, MemMap};
use m3::rc::Rc;
use m3::tiles::Tile;
use m3::util::math;

use crate::rerrno;
use crate::rerror;

use super::Resources;

pub struct MemMod {
    mcap: MemCap,
    addr: GlobAddr,
    size: GlobOff,
    reserved: bool,
}

impl MemMod {
    pub fn new(mcap: MemCap, addr: GlobAddr, size: GlobOff, reserved: bool) -> Self {
        MemMod {
            mcap,
            addr,
            size,
            reserved,
        }
    }

    pub fn mgate(&self) -> &MemCap {
        &self.mcap
    }

    pub fn addr(&self) -> GlobAddr {
        self.addr
    }

    pub fn capacity(&self) -> GlobOff {
        self.size
    }
}

impl fmt::Debug for MemMod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MemMod[sel: {}, res: {}, addr: {}, size: {} MiB]",
            self.mcap.sel(),
            self.reserved,
            self.addr,
            self.size / (1024 * 1024),
        )
    }
}

#[derive(Default)]
pub struct MemoryManager {
    mods: Vec<(Rc<MemMod>, MemMap<GlobOff>)>,
}

impl MemoryManager {
    pub fn mods(&self) -> impl Iterator<Item = &Rc<MemMod>> {
        self.mods.iter().map(|(m, _map)| m)
    }

    pub fn add(&mut self, m: Rc<MemMod>) {
        let off = m.addr().offset();
        let cap = m.capacity();
        self.mods.push((m, MemMap::new(off, cap)));
    }

    pub fn capacity(&self) -> GlobOff {
        self.mods
            .iter()
            .filter(|(m, _map)| !m.reserved)
            .fold(0, |total, (m, _map)| total + m.capacity())
    }

    pub fn available(&self) -> GlobOff {
        self.mods
            .iter()
            .filter(|(m, _map)| !m.reserved)
            .fold(0, |total, (_m, map)| total + map.size().0)
    }

    pub fn find_mem(&self, addr: GlobAddr, size: GlobOff, perm: Perm) -> anyhow::Result<MemSlice> {
        for (m, _map) in &self.mods {
            if addr.tile() == m.addr.tile()
                && addr.offset() >= m.addr.offset()
                && addr.offset() + size <= m.addr.offset() + m.capacity()
            {
                return Ok(MemSlice::new(
                    m.clone(),
                    addr.offset() - m.addr.offset(),
                    size,
                    perm,
                ));
            }
        }
        Err(rerrno(Code::InvArgs).context(format!("find memory with {}:{} {:?}", addr, size, perm)))
    }

    pub fn alloc_mem(&mut self, mut size: GlobOff) -> anyhow::Result<MemSlice> {
        size = math::round_up(size, cfg::PAGE_SIZE as GlobOff);
        for (m, map) in &mut self.mods {
            if m.reserved {
                continue;
            }
            if let Some(addr) = map.allocate(size, 1) {
                return Ok(MemSlice::new(
                    m.clone(),
                    addr - m.addr().offset(),
                    size,
                    Perm::RWX,
                ));
            }
        }
        Err(rerrno(Code::NoSpace).context(format!("allocate {}", size)))
    }

    pub fn alloc_pool(&mut self, mut size: GlobOff, exclusive: bool) -> anyhow::Result<MemPool> {
        assert!(!exclusive || size.is_power_of_two());

        let mut res = MemPool::new(exclusive);
        size = math::round_up(size, cfg::PAGE_SIZE as GlobOff);

        for (m, map) in &mut self.mods {
            if m.reserved {
                continue;
            }

            let align = if exclusive { size } else { 1 };
            if let Some(addr) = map.allocate(size, align) {
                let sl = MemSlice::new(m.clone(), addr - m.addr().offset(), size, Perm::RWX);
                res.add(sl)?;
                return Ok(res);
            }

            if let Some(max_cont) = map.largest_contiguous() {
                let align = if exclusive { max_cont } else { 1 };
                if let Some(addr) = map.allocate(max_cont, align) {
                    let sl =
                        MemSlice::new(m.clone(), addr - m.addr().offset(), max_cont, Perm::RWX);
                    res.add(sl)?;
                    size -= max_cont;
                }
            }
        }

        if size == 0 {
            Ok(res)
        }
        else {
            Err(rerrno(Code::NoSpace).context(format!("allocate pool {}, {}", size, exclusive)))
        }
    }
}

pub struct MemSlice {
    mem: Rc<MemMod>,
    offset: GlobOff,
    size: GlobOff,
    map: MemMap<GlobOff>,
    perm: Perm,
}

impl MemSlice {
    pub fn new(mem: Rc<MemMod>, offset: GlobOff, size: GlobOff, perm: Perm) -> Self {
        MemSlice {
            mem,
            offset,
            size,
            map: MemMap::new(offset, size),
            perm,
        }
    }

    pub fn in_reserved_mem(&self) -> bool {
        self.mem.reserved
    }

    pub fn derive(&self) -> anyhow::Result<MemCap> {
        self.mem
            .mcap
            .derive(self.offset, self.size, self.perm)
            .map_err(rerror)
    }

    pub fn derive_with(&self, off: GlobOff, size: GlobOff) -> anyhow::Result<MemCap> {
        self.mem
            .mcap
            .derive(self.offset + off, size, self.perm)
            .map_err(rerror)
    }

    pub fn allocate(&mut self, size: GlobOff, align: GlobOff) -> anyhow::Result<GlobOff> {
        self.map.allocate(size, align).ok_or_else(|| {
            rerrno(Code::OutOfMem)
                .context(format!("memory map has no space for {}, {}", size, align))
        })
    }

    pub fn free(&mut self, addr: GlobOff, size: GlobOff) {
        self.map.free(addr, size);
    }

    pub fn addr(&self) -> GlobAddr {
        self.mem.addr + self.offset
    }

    pub fn sel(&self) -> Selector {
        self.mem.mcap.sel()
    }

    pub fn capacity(&self) -> GlobOff {
        self.size
    }

    pub fn available(&self) -> GlobOff {
        self.map.size().0
    }
}

impl fmt::Display for MemSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MemSlice[{} .. {}, {:?}]",
            self.mem.addr + self.offset,
            self.mem.addr + self.offset + (self.size - 1),
            self.perm,
        )
    }
}

impl fmt::Debug for MemSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MemSlice[mod: {:?}, available: {} MiB, perm: {:?}, map: {:?}]",
            self.mem,
            self.map.size().0 / (1024 * 1024),
            self.perm,
            self.map
        )
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Allocation {
    slice_id: usize,
    addr: GlobOff,
    size: GlobOff,
}

impl Allocation {
    pub fn new(slice_id: usize, addr: GlobOff, size: GlobOff) -> Self {
        Allocation {
            slice_id,
            addr,
            size,
        }
    }

    pub fn slice_id(&self) -> usize {
        self.slice_id
    }

    pub fn addr(&self) -> GlobOff {
        self.addr
    }

    pub fn size(&self) -> GlobOff {
        self.size
    }
}

impl fmt::Debug for Allocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Alloc[slice={}, addr={:#x}, size={:#x}]",
            self.slice_id, self.addr, self.size
        )
    }
}

pub struct MemPool {
    exclusive: bool,
    slices: Vec<MemSlice>,
}

impl MemPool {
    pub fn new(exclusive: bool) -> Self {
        Self {
            exclusive,
            slices: Vec::new(),
        }
    }

    pub fn slices(&self) -> &Vec<MemSlice> {
        &self.slices
    }

    pub fn capacity(&self) -> GlobOff {
        self.slices.iter().fold(0, |total, m| total + m.capacity())
    }

    pub fn available(&self) -> GlobOff {
        self.slices.iter().fold(0, |total, m| total + m.available())
    }

    pub fn mem_cap(&self, idx: usize) -> Selector {
        self.slices[idx].mem.mcap.sel()
    }

    fn add(&mut self, s: MemSlice) -> anyhow::Result<()> {
        let slice = if self.exclusive {
            // derive a new memory cap now for exactly that slice so that we can simply make that
            // exclusive later without needing to change it.
            let mem = s.derive()?;
            MemSlice::new(
                Rc::new(MemMod::new(
                    mem,
                    s.addr(),
                    s.capacity(),
                    s.in_reserved_mem(),
                )),
                0,
                s.capacity(),
                Perm::RW,
            )
        }
        else {
            s
        };
        self.slices.push(slice);
        Ok(())
    }

    pub fn make_exclusive(&mut self, res: &Resources, user_tile: &Tile) -> anyhow::Result<()> {
        assert!(self.exclusive);
        for s in &self.slices {
            let mem_tile = res.tiles().find_by_id(
                s.mem
                    .mcap
                    .region()
                    .map_err(|e| rerror(e).context("exclusive MemGate region"))?
                    .0
                    .tile(),
            )?;
            s.mem
                .mcap
                .make_exclusive(&mem_tile, user_tile, true)
                .map_err(|e| rerror(e).context("make MemGate exclusive"))?;
        }
        Ok(())
    }

    pub fn allocate_slice(&mut self, size: GlobOff) -> anyhow::Result<MemSlice> {
        let alloc = self.allocate(size, None)?;
        let slice = &self.slices[alloc.slice_id];
        Ok(MemSlice::new(
            slice.mem.clone(),
            alloc.addr,
            alloc.size,
            Perm::RWX,
        ))
    }

    pub fn allocate(
        &mut self,
        size: GlobOff,
        align: Option<GlobOff>,
    ) -> anyhow::Result<Allocation> {
        let align = align.unwrap_or(if size >= cfg::LPAGE_SIZE as GlobOff {
            cfg::LPAGE_SIZE as GlobOff
        }
        else {
            cfg::PAGE_SIZE as GlobOff
        });

        for (id, s) in self.slices.iter_mut().enumerate() {
            if s.mem.reserved {
                continue;
            }

            if let Ok(addr) = s.allocate(size, align) {
                let alloc = Allocation::new(id, addr, size);
                log!(LogFlags::ResMngMem, "Allocated {:?}", alloc);
                return Ok(alloc);
            }
        }
        Err(rerrno(Code::OutOfMem).context(format!("allocate {} from pool", size)))
    }

    pub fn free(&mut self, alloc: Allocation) {
        let s = &mut self.slices[alloc.slice_id];
        log!(LogFlags::ResMngMem, "Freeing {:?}", alloc);
        if !s.mem.reserved {
            s.free(alloc.addr, alloc.size);
        }
    }
}

impl fmt::Debug for MemPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "MemPool[size: {} MiB, available: {} MiB, slices: [",
            self.capacity() / (1024 * 1024),
            self.available() / (1024 * 1024)
        )?;
        for m in &self.slices {
            writeln!(f, "  {:?}", m)?;
        }
        write!(f, "]]")
    }
}
