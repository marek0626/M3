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

use base::cell::{LazyStaticRefCell, RefMut, StaticCell};
use base::col::Vec;
use base::kif::tilemux::QuotaId;
use base::kif::{self, TileAttr, TileType};
use base::tcu::{GenId, TileId};
use thread::{StrongRc, TempRc};

use crate::cap::{TileObject, TileQuota};
use crate::mem::MemType;
use crate::platform;
use crate::tiles::{ExRegs, TileMux};
use crate::{ktcu, mem};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    RUNNING,
    DEINIT,
    SHUTDOWN,
}

const KERNEL_EPREGS: usize = 4;

struct PerTile<T> {
    objs: Vec<Vec<Option<T>>>,
}

impl<T> Default for PerTile<T> {
    fn default() -> Self {
        Self { objs: Vec::new() }
    }
}

impl<T> PerTile<T> {
    fn get(&self, tile: TileId) -> Option<&T> {
        self.objs[tile.chip() as usize]
            .get(tile.tile() as usize)
            .unwrap()
            .as_ref()
    }

    fn get_mut(&mut self, tile: TileId) -> Option<&mut T> {
        self.objs[tile.chip() as usize]
            .get_mut(tile.tile() as usize)
            .unwrap()
            .as_mut()
    }

    fn add(&mut self, tile: TileId, obj: T) {
        let cid = tile.chip() as usize;
        let tid = tile.tile() as usize;
        if cid >= self.objs.len() {
            assert_eq!(cid, self.objs.len());
            self.objs.push(Vec::new());
        }
        while tid != self.objs[cid].len() {
            self.objs[cid].push(None);
        }

        self.objs[cid].push(Some(obj));
    }
}

static TILEMUXS: LazyStaticRefCell<PerTile<TileMux>> = LazyStaticRefCell::default();
static EXREGS: LazyStaticRefCell<PerTile<ExRegs>> = LazyStaticRefCell::default();
static TILEGENS: LazyStaticRefCell<PerTile<GenId>> = LazyStaticRefCell::default();
static EPMTILE: LazyStaticRefCell<StrongRc<TileObject>> = LazyStaticRefCell::default();
static STATE: StaticCell<State> = StaticCell::new(State::RUNNING);

pub fn state() -> State {
    STATE.get()
}

pub fn init() {
    deprivilege_tiles();

    let mut exregs = PerTile::default();
    let mut tiles = PerTile::default();
    let mut tilegens = PerTile::default();
    for tile in platform::all_tiles() {
        if tile != platform::kernel_tile() {
            if platform::tile_desc(tile).tile_type() == TileType::Comp {
                tiles.add(tile, TileMux::new(tile));
            }
            exregs.add(tile, ExRegs::new(tile));
        }
        tilegens.add(tile, 0);
    }
    TILEMUXS.set(tiles);
    EXREGS.set(exregs);
    TILEGENS.set(tilegens);

    // create tile object for EP memory
    let mem = mem::borrow_mut();
    let epmem = mem
        .mods()
        .iter()
        .find(|m| m.mem_type() == MemType::EPS)
        .unwrap();
    let epmtile = epmem.addr().tile();
    let tileobj = TileObject::new(
        epmtile,
        TileQuota::new(0),
        TileQuota::new(platform::tile_desc(epmtile).exclusive_regions()),
        QuotaId::default(),
        QuotaId::default(),
        false,
    );
    EPMTILE.set(tileobj);
}

pub fn deinit_async() {
    assert_eq!(STATE.get(), State::RUNNING);
    STATE.set(State::DEINIT);

    let mut rot_tile = None;

    for tile in platform::user_tiles() {
        // ignore the tiles that are already shut down
        if tilemux(tile).is_initialized() {
            // reset the RoT last
            if platform::tile_desc(tile).attr().contains(TileAttr::ROT) {
                rot_tile = Some(tile);
            }
            else {
                TileMux::reset_async(tile, None, None, None, false).unwrap();
            }
        }
    }

    if let Some(rot_tile) = rot_tile.take() {
        TileMux::reset_async(rot_tile, None, None, None, false).unwrap();
    }

    STATE.set(State::SHUTDOWN);
}

pub fn ep_mem_tile() -> TempRc<TileObject> {
    TempRc::new(EPMTILE.borrow_mut().clone())
}

pub fn tilegen(tile: TileId) -> GenId {
    if TILEGENS.is_some() {
        *TILEGENS.borrow().get(tile).unwrap()
    }
    else {
        // during initialization we don't reset any tile, so that the generation is always 0
        0
    }
}

pub fn inc_tilegen(tile: TileId) {
    let mut gens = TILEGENS.borrow_mut();
    *gens.get_mut(tile).unwrap() += 1;
}

pub fn tilemux(tile: TileId) -> RefMut<'static, TileMux> {
    RefMut::map(TILEMUXS.borrow_mut(), |tiles| {
        tiles
            .get_mut(tile)
            .unwrap_or_else(|| panic!("No TileMux for tile {}", tile))
    })
}

pub fn exregs(tile: TileId) -> RefMut<'static, ExRegs> {
    RefMut::map(EXREGS.borrow_mut(), |exregs| exregs.get_mut(tile).unwrap())
}

pub fn new_tile_obj(tile: TileId) -> StrongRc<TileObject> {
    match platform::tile_desc(tile).tile_type() {
        TileType::Comp => tilemux(tile).new_tile_obj(),
        TileType::Mem => {
            let (num, derived) = if tile == EPMTILE.borrow().tile() {
                let epmtile = EPMTILE.borrow_mut().clone();
                let num = match epmtile.exregs_quota().total() {
                    0 => 0,
                    n => n - KERNEL_EPREGS,
                };
                epmtile.alloc_exreg(num);
                (num, true)
            }
            else {
                (platform::tile_desc(tile).exclusive_regions(), false)
            };

            TileObject::new(
                tile,
                TileQuota::new(0),
                TileQuota::new(num),
                QuotaId::default(),
                QuotaId::default(),
                derived,
            )
        },
    }
}

pub fn find_tile(tiledesc: &kif::TileDesc) -> Option<TileId> {
    platform::user_tiles().find(|&tile| {
        platform::tile_desc(tile).isa() == tiledesc.isa()
            && platform::tile_desc(tile).tile_type() == tiledesc.tile_type()
    })
}

fn deprivilege_tiles() {
    for tile in platform::user_tiles() {
        // do not deprivilege the RoT (it needs to change its EPs to have access to TCUs after a
        // tile was reset so that its generation changed)
        if !platform::tile_desc(tile).attr().contains(TileAttr::ROT) {
            ktcu::deprivilege_tile(tile).expect("Unable to deprivilege tile");
        }
    }
}
