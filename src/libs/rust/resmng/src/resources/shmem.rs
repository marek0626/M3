/*
 * Copyright (C) 2021 Nils Asmussen, Barkhausen Institut
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

use anyhow::{anyhow, Context};
use m3::col::{String, Vec};
use m3::com::MemCap;
use m3::io::LogFlags;
use m3::rc::Rc;
use m3::tiles::Tile;
use m3::{format, log};

use super::memory::MemSlice;

struct SharedMem {
    slice: MemSlice,
    name: String,
    rem_users: usize,
}

#[derive(Default)]
pub struct SharedMemManager {
    shmems: Vec<SharedMem>,
}

impl SharedMemManager {
    pub const fn new() -> Self {
        Self { shmems: Vec::new() }
    }

    pub fn add_mem(&mut self, slice: MemSlice, name: String, users: usize) {
        log!(
            LogFlags::ResMngShMem,
            "Created shmem '{}' of size {}b for {} users",
            name,
            slice.capacity(),
            users
        );
        self.shmems.push(SharedMem {
            slice,
            name,
            rem_users: users,
        });
    }

    pub fn get(&self, name: &str) -> Option<&MemCap> {
        let shmem = self.shmems.iter().find(|shmem| shmem.name == name)?;
        Some(shmem.slice.capability())
    }

    pub fn acquire_for_tile(&mut self, name: &str, tile: &Rc<Tile>) -> anyhow::Result<&MemCap> {
        let shmem = self
            .shmems
            .iter_mut()
            .find(|shmem| shmem.name == name)
            .ok_or_else(|| anyhow!("No shared memory with name '{}'", name))?;

        assert!(shmem.rem_users > 0);
        shmem.rem_users -= 1;

        log!(
            LogFlags::ResMngShMem,
            "Acquired shmem '{}' for tile {}",
            name,
            tile.id()
        );

        shmem
            .slice
            .make_exclusive_for(tile, shmem.rem_users == 0)
            .context(format!("making shared memory '{}' exclusive", name))?;
        Ok(shmem.slice.capability())
    }
}
