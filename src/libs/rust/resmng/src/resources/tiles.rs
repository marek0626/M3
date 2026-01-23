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

use anyhow::Context;

use m3::cell::{Cell, Ref, RefCell, RefMut};
use m3::col::Vec;
use m3::com::{GateCap, MemCap, MemGate};
use m3::errors::{Code, Error};
use m3::io::{LogFlags, Read};
use m3::kif::{Perm, TileDesc};
use m3::log;
use m3::mem::GlobOff;
use m3::rc::Rc;
use m3::syscalls;
use m3::tcu::{EpId, TileId};
use m3::tiles::Tile;
use m3::time::TimeDuration;
use m3::util::math;
use m3::vec;
use m3::vfs::{Seek, SeekMode};
use m3::{cfg, format};

use crate::resources::memory::Allocation;
use crate::{rerrno, rerror};

// PMP EPs start at 1, because 0 is reserved for TileMux
const FIRST_FREE_PMP_EP: EpId = 1;

// The hardcoded location of the DTB as expected by bbl
const DTB_OFFSET: usize = 0x1FF000;

#[derive(Debug)]
struct TileMem {
    mem: MemGate,
    alloc: Option<Allocation>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum State {
    On,
    Off,
}

#[derive(Debug)]
pub struct TileState {
    state: State,
    tile: Rc<Tile>,
    next_pmp_ep: EpId,
    pmp_regions: Vec<(MemCap, usize)>,
    mux: Option<TileMem>,
}

struct MuxBootMod<'a> {
    mgate: &'a MemGate,
    off: GlobOff,
}

impl Seek for MuxBootMod<'_> {
    fn seek(&mut self, pos: usize, mode: SeekMode) -> Result<usize, Error> {
        assert_eq!(mode, SeekMode::Set);
        self.off = pos as GlobOff;
        Ok(pos)
    }
}

impl Read for MuxBootMod<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.mgate.read(buf, self.off)?;
        self.off += buf.len() as GlobOff;
        Ok(buf.len())
    }
}

impl TileState {
    fn new(tile: Rc<Tile>) -> Self {
        Self {
            state: State::Off,
            tile,
            next_pmp_ep: FIRST_FREE_PMP_EP,
            pmp_regions: Vec::new(),
            mux: None,
        }
    }

    pub fn add_mem_region(
        &mut self,
        mcap: MemCap,
        size: usize,
        set: bool,
        overwrite: bool,
    ) -> anyhow::Result<()> {
        if set {
            loop {
                match syscalls::tile_set_pmp(
                    self.tile.sel(),
                    mcap.sel(),
                    self.next_pmp_ep,
                    overwrite,
                ) {
                    Err(e) if e.code() == Code::Exists && !overwrite => self.next_pmp_ep += 1,
                    Err(e) => return Err(rerror(e).context("set PMP region")),
                    Ok(_) => break,
                }
            }

            self.next_pmp_ep += 1;
        }
        self.pmp_regions.push((mcap, size));
        Ok(())
    }

    pub fn inherit_mem_regions(&mut self, tile: &TileUsage) -> anyhow::Result<()> {
        for (mgate, size) in tile.state().pmp_regions.iter() {
            self.add_mem_region(
                mgate
                    .derive(0, *size as GlobOff, Perm::RWX)
                    .map_err(|e| rerror(e).context("derive inherited PMP region"))?,
                *size,
                true,
                true,
            )?;
        }
        Ok(())
    }

    fn copy_data(
        buf: &mut [u8],
        src: &MemGate,
        dst: &MemGate,
        src_off: usize,
        dst_off: usize,
        size: usize,
    ) -> anyhow::Result<()> {
        let mut pos = 0;
        while pos < size {
            let amount = (size - pos).min(buf.len());
            src.read(&mut buf[0..amount], (src_off + pos) as GlobOff)
                .map_err(|e| {
                    rerror(e).context(format!("read {} from {}", amount, src_off + pos))
                })?;
            dst.write(&buf[0..amount], (dst_off + pos) as GlobOff)
                .map_err(|e| rerror(e).context(format!("write {} to {}", amount, dst_off + pos)))?;
            pos += amount;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn load_mux<A, M>(
        &mut self,
        name: &str,
        mem_size: usize,
        ep_count: usize,
        initrd: Option<&str>,
        dtb: Option<&str>,
        mut alloc_mem: A,
        mut get_mod: M,
    ) -> anyhow::Result<()>
    where
        A: FnMut(usize) -> anyhow::Result<(MemGate, Option<Allocation>)>,
        M: FnMut(&str) -> anyhow::Result<MemGate>,
    {
        if self.state == State::On {
            return Ok(());
        }

        let mux = match self.tile.memory() {
            Ok(mem) => TileMem {
                mem: mem
                    .activate()
                    .map_err(|e| rerror(e).context("Activate tile memory cap"))?,
                alloc: None,
            },
            Err(_) => {
                let (mem, alloc) = alloc_mem(mem_size)?;
                TileMem { mem, alloc }
            },
        };
        let mux_elf = get_mod(name).with_context(|| format!("get mod {}", name))?;

        // load multiplexer ELF into the memory region
        let mut mux_bmod = MuxBootMod {
            mgate: &mux_elf,
            off: 0,
        };
        self.tile
            .load_mux(name, &mut mux_bmod, &mux.mem)
            .map_err(|e| rerror(e).context("load mux"))?;

        let mut buf = vec![0u8; 4096];

        // load initrd to the end of the memory region
        if let Some(initrd) = initrd {
            let rd_mod = get_mod(initrd).with_context(|| format!("getting mod {}", initrd))?;
            let rd_size = rd_mod
                .region()
                .map_err(|e| rerror(e).context("initrd region"))?
                .1 as usize;
            let rd_start = mem_size - math::round_up(rd_size, cfg::PAGE_SIZE);

            log!(
                LogFlags::ResMngTiles,
                "Loading initrd '{}' with {}b to {:#x}",
                initrd,
                rd_size,
                self.tile.desc().mem_offset() + rd_start
            );

            Self::copy_data(&mut buf, &rd_mod, &mux.mem, 0, rd_start, rd_size)
                .with_context(|| "copying initrd")?;
        }

        // load dtb to the expected location
        if let Some(dtb) = dtb {
            let dtb_mod = get_mod(dtb).with_context(|| format!("getting mod {}", dtb))?;
            let dtb_size = dtb_mod
                .region()
                .map_err(|e| rerror(e).context("DTB region"))?
                .1 as usize;
            // the payload of bbl starts one page behind the dtb
            assert!(dtb_size <= cfg::PAGE_SIZE);

            log!(
                LogFlags::ResMngTiles,
                "Loading dtb '{}' with {}b to {:#x}",
                dtb,
                dtb_size,
                self.tile.desc().mem_offset() + DTB_OFFSET
            );

            Self::copy_data(&mut buf, &dtb_mod, &mux.mem, 0, DTB_OFFSET, dtb_size)
                .with_context(|| "copying DTB")?;
        }

        self.start(Some(&mux.mem), ep_count)
            .with_context(|| "starting mux")?;

        self.mux = Some(mux);
        Ok(())
    }

    pub fn unload_mux<F>(&mut self, free: F) -> anyhow::Result<()>
    where
        F: FnOnce(Allocation),
    {
        if self.state == State::Off {
            return Ok(());
        }

        self.stop()?;

        if let Some(mux) = self.mux.take() {
            if let Some(alloc) = mux.alloc {
                free(alloc);
            }
        }

        Ok(())
    }

    pub fn start(&mut self, mem: Option<&MemGate>, ep_count: usize) -> anyhow::Result<()> {
        self.tile
            .start(mem, ep_count)
            .map_err(|e| rerror(e).context("start tile"))?;
        self.state = State::On;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        // reset the tile before we drop the MemGate for its PMP EP
        self.tile
            .stop()
            .map_err(|e| rerror(e).context(format!("reset tile {} for stop", self.tile.id())))?;
        self.state = State::Off;
        Ok(())
    }
}

impl Drop for TileState {
    fn drop(&mut self) {
        self.unload_mux(|_alloc| panic!("Mux memory not freed before dropping tile"))
            .unwrap();
    }
}

#[derive(Clone, Debug)]
pub struct TileUsage {
    idx: Option<usize>,
    state: Rc<RefCell<TileState>>,
    tile: Rc<Tile>,
}

impl TileUsage {
    fn new(idx: usize, tile: Rc<Tile>) -> Self {
        Self {
            idx: Some(idx),
            state: Rc::new(RefCell::new(TileState::new(tile.clone()))),
            tile,
        }
    }

    pub fn new_obj(tile: Rc<Tile>) -> Self {
        Self {
            idx: None,
            state: Rc::new(RefCell::new(TileState::new(tile.clone()))),
            tile,
        }
    }

    pub fn index(&self) -> Option<usize> {
        self.idx
    }

    pub fn tile_id(&self) -> TileId {
        self.tile.id()
    }

    pub fn tile_obj(&self) -> &Rc<Tile> {
        &self.tile
    }

    pub fn state(&self) -> Ref<'_, TileState> {
        self.state.borrow()
    }

    pub fn state_mut(&mut self) -> RefMut<'_, TileState> {
        self.state.borrow_mut()
    }

    pub fn derive(
        &self,
        eps: Option<usize>,
        time: Option<TimeDuration>,
        pts: Option<usize>,
    ) -> anyhow::Result<TileUsage> {
        let tile = self
            .tile_obj()
            .derive(eps, None, time, pts)
            .map_err(|e| rerror(e).context("tile derive"))?;
        let _quota = tile.quota().unwrap();
        log!(
            LogFlags::ResMngTiles,
            "Deriving {}: (eps={:?}, time={:?}, pts={:?})",
            self.tile_id(),
            _quota.endpoints(),
            _quota.time(),
            _quota.page_tables(),
        );
        Ok(TileUsage {
            idx: self.idx,
            state: self.state.clone(),
            tile,
        })
    }
}

struct ManagedTile {
    id: TileId,
    tile: Rc<Tile>,
    users: Cell<u32>,
}

impl ManagedTile {
    fn add_user(&self) -> u32 {
        let old = self.users.get();
        self.users.set(old + 1);
        old
    }

    fn remove_user(&self) -> u32 {
        self.users.replace(self.users.get() - 1)
    }
}

#[derive(Default)]
pub struct TileManager {
    tiles: Vec<ManagedTile>,
}

impl TileManager {
    pub fn count(&self) -> usize {
        self.tiles.len()
    }

    pub fn get(&self, idx: usize) -> Rc<Tile> {
        self.tiles[idx].tile.clone()
    }

    pub fn add(&mut self, tile: Rc<Tile>) {
        self.tiles.push(ManagedTile {
            id: tile.id(),
            tile,
            users: Cell::from(0),
        });
    }

    pub fn add_user(&self, usage: &TileUsage) {
        if let Some(idx) = usage.idx {
            if self.tiles[idx].add_user() == 0 {
                log!(
                    LogFlags::ResMngTiles,
                    "Allocating {}: {:?}",
                    self.tiles[idx].id,
                    self.tiles[idx].tile.desc(),
                );
            }
        }
    }

    pub fn remove_user(&self, usage: &TileUsage) {
        if let Some(idx) = usage.idx {
            if self.tiles[idx].remove_user() == 1 {
                log!(
                    LogFlags::ResMngTiles,
                    "Freeing {}: {:?}",
                    self.tiles[idx].id,
                    self.tiles[idx].tile.desc()
                );
            }
        }
    }

    pub fn find_by_id(&self, id: TileId) -> anyhow::Result<Rc<Tile>> {
        for tile in &self.tiles {
            if tile.id == id {
                return Ok(tile.tile.clone());
            }
        }
        Err(rerrno(Code::NotFound).context(format!("find tile {}", id)))
    }

    pub fn find(&self, desc: TileDesc) -> anyhow::Result<TileUsage> {
        for (id, tile) in self.tiles.iter().enumerate() {
            if tile.users.get() == 0
                && tile.tile.desc().isa() == desc.isa()
                && tile.tile.desc().tile_type() == desc.tile_type()
                && (tile.tile.desc().attr() & desc.attr()) == desc.attr()
            {
                return Ok(TileUsage::new(id, tile.tile.clone()));
            }
        }
        Err(rerrno(Code::NotFound).context(format!("find tile with {:?}", desc)))
    }

    pub fn find_with_attr(&self, base: TileDesc, attr: &str) -> anyhow::Result<TileUsage> {
        for props in attr.split('|') {
            if let Ok(usage) = self.find(base.with_properties(props)) {
                return Ok(usage);
            }
        }
        Err(rerrno(Code::NotFound).context(format!("find tile with {:?} and {}", base, attr)))
    }
}
