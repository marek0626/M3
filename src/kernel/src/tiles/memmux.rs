/*
 * Copyright (C) 2024 Nils Asmussen, Barkhausen Institut
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

use base::col::{BitArray, Vec};
use base::errors::{Code, Error};
use base::tcu::TileId;
use thread::{Downgradable, TempRc, Upgradable, WeakRc};

use crate::cap::{MGateObject, TileObject};
use crate::{ktcu, platform};

struct ExclRegion {
    idx: usize,
    utile_id: TileId,
    mtile_id: TileId,
    mgate: WeakRc<MGateObject>,
    mtile: WeakRc<TileObject>,
}

pub struct MemMux {
    tile: TileId,
    free: BitArray,
    exregs: Vec<ExclRegion>,
}

impl MemMux {
    pub fn new(tile: TileId) -> Self {
        Self {
            tile,
            free: BitArray::new(platform::tile_desc(tile).exclusive_regions()),
            exregs: Vec::new(),
        }
    }

    pub fn add(
        &mut self,
        mgate: TempRc<MGateObject>,
        mem_tile: TempRc<TileObject>,
        user_tile: TempRc<TileObject>,
    ) -> anyhow::Result<()> {
        assert!(mem_tile.tile() == self.tile);

        if mem_tile.exregs_quota().left() == 0 {
            return Err(anyhow!(Error::new(Code::NoSpace)).context("Exclusive-region quota"));
        }
        mgate.make_exclusive(&user_tile)?;

        let idx = self.free.first_clear();

        ktcu::set_excl_region(
            mem_tile.tile(),
            user_tile.tile(),
            idx,
            mgate.offset(),
            mgate.size(),
            mgate.perms(),
        )?;

        mem_tile.alloc_exreg(1);
        self.exregs.push(ExclRegion {
            idx,
            utile_id: user_tile.tile(),
            mtile_id: mem_tile.tile(),
            mgate: mgate.downgrade_store(),
            mtile: mem_tile.downgrade_store(),
        });
        self.free.set(idx);

        Ok(())
    }

    pub fn invalidate(&mut self, tile: TileId) {
        self.exregs.retain_mut(|e| {
            if e.utile_id != tile {
                return true;
            }

            if let Some(mgate) = e.mgate.upgrade() {
                mgate.inval_exclusive();
            }
            if let Some(mtile) = e.mtile.upgrade() {
                mtile.free_exregs(1);
            }

            ktcu::invalidate_excl_region(e.mtile_id, e.idx).unwrap();

            false
        });
    }
}
