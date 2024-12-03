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

use anyhow::Context;

use base::cell::LazyStaticRefCell;
use base::cfg;
use base::col::{String, ToString, Vec};
use base::errors::Code;
use base::io::LogFlags;
use base::kif::{self, CapSel, Perm};
use base::log;
use base::mem::{GlobAddr, GlobOff};
use base::tcu::{self, ActId, TileId};
use base::util::math;
use base::vec;

use thread::{Downgradable, StrongRc, TempRc, Upgradable, WeakRc};

use crate::args;
use crate::cap::{Capability, KMemObject, MGateObject, RGateObject, SelRange, TileObject};
use crate::kerrno;
use crate::mem::{self, Allocation};
use crate::platform;
use crate::tiles::{loader, tilemng, Activity, ActivityFlags, TileMux};

pub struct ActivityMng {
    acts: Vec<WeakRc<Activity>>,
    count: usize,
    next_id: tcu::ActId,
}

static INST: LazyStaticRefCell<ActivityMng> = LazyStaticRefCell::default();

pub fn init() {
    INST.set(ActivityMng {
        acts: vec![WeakRc::default(); cfg::MAX_ACTS],
        count: 0,
        next_id: 0,
    });
}

impl ActivityMng {
    pub fn count() -> usize {
        INST.borrow().count
    }

    #[inline(always)]
    pub fn activity(id: tcu::ActId) -> Option<TempRc<Activity>> {
        INST.borrow().acts[id as usize].upgrade()
    }

    fn get_id() -> anyhow::Result<tcu::ActId> {
        let mut actmng = INST.borrow_mut();
        for id in actmng.next_id..cfg::MAX_ACTS as tcu::ActId {
            if !actmng.acts[id as usize].can_upgrade() {
                actmng.next_id = id + 1;
                return Ok(id);
            }
        }

        for id in 0..actmng.next_id {
            if !actmng.acts[id as usize].can_upgrade() {
                actmng.next_id = id + 1;
                return Ok(id);
            }
        }

        Err(kerrno(Code::NoSpace))
    }

    pub fn create_activity_async(
        name: String,
        parent: Option<(ActId, CapSel)>,
        tile: TempRc<TileObject>,
        eps_start: tcu::EpId,
        kmem: TempRc<KMemObject>,
        flags: ActivityFlags,
    ) -> anyhow::Result<StrongRc<Activity>> {
        let id: tcu::ActId = Self::get_id().context("all activity ids in use")?;
        let tile_id = tile.tile();

        let act = Activity::new(name, id, parent, tile, eps_start, kmem, flags);

        log!(
            LogFlags::KernActs,
            "Created Activity {} [id={}, tile={}]",
            act.name(),
            id,
            tile_id
        );

        // note that this insertion is currently required, because when doing sidecalls to TileMux
        // we use the acts table to check whether the activity is still alive.
        {
            let mut actmng = INST.borrow_mut();
            actmng.acts[id as usize] = act.clone().downgrade_store();
            actmng.count += 1;
        }
        tilemng::tilemux(tile_id).add_activity(id);

        if flags.is_empty() {
            // if this call fails, we need to undo our actions above
            if let Err(e) = Self::init_activity_async(act.clone()) {
                // tilemux and count modifications will be reverted in Activity::drop.
                // note that this is okay, because we have not inserted the new activity into a
                // capability table and thus nobody else will have removed it from the table yet.
                let mut actmng = INST.borrow_mut();
                actmng.acts[id as usize] = WeakRc::default();
                return Err(e.context("init activity"));
            }
        }

        Ok(act)
    }

    fn init_activity_async(act: StrongRc<Activity>) -> anyhow::Result<()> {
        let tile_id = act.tile_id();

        if platform::tile_desc(tile_id).supports_tilemux() {
            let time_quota_id = act.tile().time_quota_id();
            let pt_quota_id = act.tile().pt_quota_id();

            TileMux::activity_init_async(
                tilemng::tilemux(tile_id),
                act.id(),
                time_quota_id,
                pt_quota_id,
                act.eps_start(),
            )?;
        }

        Activity::init_async(act)
    }

    pub fn start_activity_async(act_id: ActId, tile_id: TileId) -> anyhow::Result<()> {
        if platform::tile_desc(tile_id).supports_tilemux() {
            TileMux::activity_ctrl_async(
                tilemng::tilemux(tile_id),
                act_id,
                kif::tilemux::ActivityOp::Start,
            )
        }
        else {
            Ok(())
        }
    }

    pub fn stop_activity_async(act: TempRc<Activity>) -> anyhow::Result<()> {
        if platform::tile_desc(act.tile_id()).supports_tilemux() {
            let id = act.id();
            let tile_id = act.tile_id();
            drop(act);

            TileMux::activity_ctrl_async(
                tilemng::tilemux(tile_id),
                id,
                kif::tilemux::ActivityOp::Stop,
            )?;
        }
        Ok(())
    }

    pub fn start_root_async() -> anyhow::Result<()> {
        // TODO temporary
        let isa = platform::tile_desc(platform::kernel_tile()).isa();
        let tile_emem = kif::TileDesc::new(kif::TileType::Comp, isa, 0);
        let tile_imem =
            kif::TileDesc::new_with_attr(kif::TileType::Comp, isa, 0, kif::TileAttr::IMEM);

        let tile_id = tilemng::find_tile(&tile_emem)
            .unwrap_or_else(|| tilemng::find_tile(&tile_imem).unwrap());
        let tile = tilemng::tilemux(tile_id).new_tile_obj();
        let tile_desc = platform::tile_desc(tile_id);

        // allocate memory for tilemux itself
        let mux_mem = if tile_desc.has_memory() {
            tile.memory()
        }
        else {
            let mux_mem_size = cfg::FIXED_TILEMUX_MEM as GlobOff;
            mem::borrow_mut().allocate(
                mem::MemType::ROOT,
                mux_mem_size,
                cfg::PAGE_SIZE as GlobOff,
            )?
        };

        // allocate memory for the tile's EPs
        let ep_count = if tile_desc.has_internal_eps() {
            None
        }
        else {
            Some(args::get().root_eps)
        };

        // load and start tilemux
        loader::load_mux_async(tile_id, &mux_mem).expect("Unable to load TileMux");
        // note that we provide access to the entire ROOT memory pool via PMP down below and
        // therefore provide access to parts of this pool twice. that's currently required, because
        // TileMux reads PMP EP0 to discover the available memory.
        let mux_mgate = MGateObject::new(mux_mem, Perm::RWX, false);
        // ensure that the objects are not dropped during the async call
        let _mux_mgate_clone = mux_mgate.clone();
        let _tile_clone = tile.clone();
        TileMux::reset_async(
            tile_id,
            Some(TempRc::new(tile.clone())),
            Some(TempRc::new(mux_mgate)),
            ep_count,
            true,
        )
        .expect("Resetting tile for root");
        drop(_tile_clone);

        // create root activity
        let kmem = KMemObject::new(args::get().kmem - cfg::FIXED_KMEM);

        let act = Self::create_activity_async(
            "root".to_string(),
            None,
            TempRc::new(tile.clone()),
            tcu::FIRST_USER_EP,
            TempRc::new(kmem.clone()),
            ActivityFlags::IS_ROOT,
        )
        .expect("Creating root activity");

        // insert basic caps into cap space
        act.obj_caps()
            .borrow_mut()
            .insert(Capability::new(kif::SEL_KMEM, kmem))?;
        act.obj_caps()
            .borrow_mut()
            .insert(Capability::new(kif::SEL_TILE, tile))?;
        // safety: since this is root, whose caps are not revoked anyway, we are living without the
        // unique check here
        unsafe {
            act.obj_caps()
                .borrow_mut()
                .insert(Capability::new_range_unchecked(
                    SelRange::new(kif::SEL_ACT),
                    act.clone(),
                ))?;
        }

        let mut sel = kif::FIRST_FREE_SEL;
        let tile: TempRc<TileObject> = act.get_kobj(kif::SEL_TILE).unwrap();

        // boot info
        {
            let alloc = Allocation::new(platform::info_addr(), platform::info_size() as GlobOff);
            let mgate = MGateObject::new(alloc, kif::Perm::RWX, false);
            let cap = Capability::new(sel, mgate);

            act.obj_caps().borrow_mut().insert(cap).unwrap();
            sel += 1;
        }

        // serial rgate
        {
            let rgate = RGateObject::new(cfg::SERIAL_BUF_ORD, cfg::SERIAL_BUF_ORD, true);
            let cap = Capability::new(sel, rgate);
            act.obj_caps().borrow_mut().insert(cap).unwrap();
            sel += 1;
        }

        // boot modules
        for m in platform::mods() {
            let size = math::round_up(m.size as usize, cfg::PAGE_SIZE);
            let alloc = Allocation::new(GlobAddr::new(m.addr), size as GlobOff);
            let mgate = MGateObject::new(alloc, kif::Perm::RWX, false);
            let cap = Capability::new(sel, mgate);

            act.obj_caps().borrow_mut().insert(cap).unwrap();
            sel += 1;
        }

        // TILES
        for tile_id in platform::all_tiles() {
            if tile_id == platform::kernel_tile() {
                continue;
            }

            // the tile for root is special, because we already reset it (causing a state change)
            // and thus need to pass this object to userspace instead of a new one
            if tile_id == tile.tile() {
                // safety: as above (it's root)
                unsafe {
                    let cap = Capability::new_range_unchecked(
                        SelRange::new(sel),
                        TempRc::into_strong_unchecked(tile.clone()),
                    );
                    act.obj_caps().borrow_mut().insert(cap).unwrap();
                }
            }
            else {
                let cap = Capability::new(sel, tilemng::new_tile_obj(tile_id));
                act.obj_caps().borrow_mut().insert(cap).unwrap();
            }
            sel += 1;
        }
        drop(tile);

        // memory
        let mut mem_ep = 1;

        for m in mem::borrow_mut().mods() {
            if m.mem_type() != mem::MemType::KERNEL && m.mem_type() != mem::MemType::EPS {
                let alloc = Allocation::new(m.addr(), m.capacity());
                // create a derive MGateObject to prevent freeing the memory if it's of type ROOT
                let mgate_obj = MGateObject::new(alloc, kif::Perm::RWX, true);

                // we currently assume that we have enough protection EPs for all user memory regions
                assert!(mem_ep < tcu::PMEM_PROT_EPS as tcu::EpId);
                assert!(mgate_obj.size() < (1 << 30));

                // configure physical memory protection EP
                tilemng::tilemux(tile_id)
                    .config_mem_ep(
                        mem_ep,
                        kif::tilemux::ACT_ID as tcu::ActId,
                        &mgate_obj,
                        m.addr().tile(),
                    )
                    .unwrap();
                mem_ep += 1;

                if m.mem_type() != mem::MemType::ROOT {
                    // insert capability
                    let cap = Capability::new(sel, mgate_obj);
                    act.obj_caps().borrow_mut().insert(cap).unwrap();
                    sel += 1;
                }
            }
        }

        // let root know the first usable selector
        act.set_first_sel(sel);

        // go!
        Self::init_activity_async(act.clone())?;
        Activity::start_app_async(TempRc::new(act))
    }

    pub fn remove_activity(id: tcu::ActId) {
        let mut actmng = INST.borrow_mut();
        if id != 0 {
            // as this is called on Activity::drop, we should never be able to reach it here anymore
            // (the exception is root, with id 0, which we force-remove)
            assert!(actmng.acts[id as usize].upgrade().is_none());
        }
        actmng.count -= 1;
    }
}
