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
use base::kif::{self, TileType};
use base::tcu::TileId;
use thread::{StrongRc, TempRc};

use crate::cap::{TileObject, TileQuota};
use crate::mem::MemType;
use crate::platform;
use crate::tiles::{MemMux, TileMux};
use crate::{ktcu, mem};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    RUNNING,
    DEINIT,
    SHUTDOWN,
}

enum TileState {
    Compute(TileMux),
    Mem(MemMux),
}

const KERNEL_EPREGS: usize = 4;

static TILES: LazyStaticRefCell<Vec<Vec<Option<TileState>>>> = LazyStaticRefCell::default();
static EPMTILE: LazyStaticRefCell<StrongRc<TileObject>> = LazyStaticRefCell::default();
static STATE: StaticCell<State> = StaticCell::new(State::RUNNING);

pub fn state() -> State {
    STATE.get()
}

pub fn init() {
    deprivilege_tiles();

    let mut tiles = Vec::new();
    for tile in platform::all_tiles() {
        if tile == platform::kernel_tile() {
            continue;
        }

        let cid = tile.chip() as usize;
        let tid = tile.tile() as usize;
        if cid >= tiles.len() {
            assert_eq!(cid, tiles.len());
            tiles.push(Vec::new());
        }
        while tid != tiles[cid].len() {
            tiles[cid].push(None);
        }

        let state = match platform::tile_desc(tile).tile_type() {
            TileType::Comp => TileState::Compute(TileMux::new(tile)),
            TileType::Mem => TileState::Mem(MemMux::new(tile)),
        };
        tiles[cid].push(Some(state));
    }
    TILES.set(tiles);

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

    for tile in platform::user_tiles() {
        // ignore the tiles that are already shut down
        if tilemux(tile).is_initialized() {
            TileMux::reset_async(tile, None, None, None, false).unwrap();
        }
    }

    STATE.set(State::SHUTDOWN);
}

pub fn ep_mem_tile() -> TempRc<TileObject> {
    TempRc::new(EPMTILE.borrow_mut().clone())
}

pub fn tilemux(tile: TileId) -> RefMut<'static, TileMux> {
    RefMut::map(TILES.borrow_mut(), |tiles| {
        let state = tiles[tile.chip() as usize][tile.tile() as usize].as_mut();
        match state {
            Some(TileState::Compute(mux)) => mux,
            _ => panic!("No TileMux for tile {}", tile),
        }
    })
}

pub fn memmux(tile: TileId) -> RefMut<'static, MemMux> {
    RefMut::map(TILES.borrow_mut(), |tiles| {
        let state = tiles[tile.chip() as usize][tile.tile() as usize].as_mut();
        match state {
            Some(TileState::Mem(mux)) => mux,
            _ => panic!("No memory multiplexer for tile {}", tile),
        }
    })
}

pub fn new_tile_obj(tile: TileId) -> StrongRc<TileObject> {
    match platform::tile_desc(tile).tile_type() {
        TileType::Comp => tilemux(tile).new_tile_obj(),
        TileType::Mem => {
            let (num, derived) = if tile == EPMTILE.borrow().tile() {
                let epmtile = EPMTILE.borrow_mut().clone();
                let num = epmtile.exregs_quota().total() - KERNEL_EPREGS;
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
        ktcu::deprivilege_tile(tile).expect("Unable to deprivilege tile");
    }
}
