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

use base::col::{BitArray, Vec};
use base::errors::Code;
use base::kif::TileAttr;
use base::tcu::TileId;
use base::util;
use thread::{Downgradable, TempRc, Upgradable, WeakRc};

use crate::cap::{MGateObject, TileObject};
use crate::kerrno;
use crate::platform;
use crate::tiles::{tilemng, TileMux};

#[derive(Debug)]
struct ExclRegion {
    idx: usize,
    utile_id: TileId,
    mgate: WeakRc<MGateObject>,
    mtile: WeakRc<TileObject>,
}

pub struct ExRegs {
    tile: TileId,
    free: BitArray,
    exregs: Vec<ExclRegion>,
}

impl ExRegs {
    pub fn new(tile: TileId) -> Self {
        Self {
            tile,
            free: BitArray::new(platform::tile_desc(tile).exclusive_regions()),
            exregs: Vec::new(),
        }
    }

    pub fn add_async(
        mgate: TempRc<MGateObject>,
        mem_tile: TempRc<TileObject>,
        user_tile: TempRc<TileObject>,
        locked: bool,
    ) -> anyhow::Result<()> {
        let Some(rot_tile) =
            platform::user_tiles().find(|t| platform::tile_desc(*t).attr().contains(TileAttr::ROT))
        else {
            return Err(kerrno(Code::NotSup).context("No RoT"));
        };

        // not yet supported on hw
        if cfg!(not(M3_TARGET = "gem5")) {
            return Ok(());
        }

        let exregs = tilemng::exregs(mem_tile.tile());
        assert!(mem_tile.tile() == exregs.tile);

        if mem_tile.exregs_quota().left() == 0 {
            return Err(kerrno(Code::NoSpace).context("Exclusive-region quota"));
        }

        let idx = exregs.free.first_clear();

        let mtile = mem_tile.tile();
        let utile = user_tile.tile();
        let ugen = tilemng::tilegen(utile);
        let (addr, size, perms) = (mgate.offset(), mgate.size(), mgate.perms());

        let mgate_weak = mgate.downgrade_asyn();
        let mtile_weak = mem_tile.downgrade_asyn();
        let utile_weak = user_tile.downgrade_asyn();
        drop(exregs);

        let tilemux = tilemng::tilemux(rot_tile);
        TileMux::exreg_add_async(tilemux, mtile, idx, utile, ugen, addr, size, perms, locked)?;

        let mgate = mgate_weak
            .upgrade()
            .ok_or_else(|| kerrno(Code::ObjectGone))?;
        let utile = utile_weak
            .upgrade()
            .ok_or_else(|| kerrno(Code::ObjectGone))?;
        let mtile = mtile_weak
            .upgrade()
            .ok_or_else(|| kerrno(Code::ObjectGone))?;

        let mut exregs = tilemng::exregs(mtile.tile());
        mgate.make_exclusive();
        mtile.alloc_exreg(1);
        exregs.exregs.push(ExclRegion {
            idx,
            utile_id: utile.tile(),
            mgate: mgate.downgrade_store(),
            mtile: mtile.downgrade_store(),
        });
        exregs.free.set(idx);

        Ok(())
    }

    pub fn has_access(&self, tile: TileId, mgate: &MGateObject) -> bool {
        for ereg in &self.exregs {
            if let Some(ereg_gate) = ereg.mgate.upgrade() {
                assert_eq!(ereg_gate.addr().tile(), mgate.addr().tile());
                if ereg.utile_id == tile
                    && util::math::overlaps(
                        ereg_gate.addr().offset(),
                        ereg_gate.addr().offset() + ereg_gate.size(),
                        mgate.addr().offset(),
                        mgate.addr().offset() + mgate.size(),
                    )
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn invalidate_async(mtile: TileId, tile: TileId) {
        let Some(rot_tile) =
            platform::user_tiles().find(|t| platform::tile_desc(*t).attr().contains(TileAttr::ROT))
        else {
            return;
        };

        loop {
            let mut exregs = tilemng::exregs(mtile);

            if let Some(idx) = exregs.exregs.iter().position(|e| e.utile_id == tile) {
                let e = &mut exregs.exregs[idx];

                if let Some(mgate) = e.mgate.upgrade() {
                    mgate.inval_exclusive();
                }
                if let Some(mtile) = e.mtile.upgrade() {
                    mtile.free_exregs(1);
                }

                let exreg_idx = e.idx;
                exregs.exregs.remove(idx);
                exregs.free.clear(idx);
                drop(exregs);

                let tilemux = tilemng::tilemux(rot_tile);
                // if we've already "shutdown" the RoT tile, don't even try. note that this
                // currently means that we will not get rid of exregs that are owned by the RoT as
                // this tile is never reset so that we do not even consider removing its exregs.
                if tilemux.is_initialized() {
                    TileMux::exreg_rem_async(tilemux, mtile, exreg_idx).unwrap();
                }
            }
            else {
                break;
            }
        }
    }
}
