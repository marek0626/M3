/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
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

//! The TCU's external interface

use num_enum::IntoPrimitive;

use bitflags::bitflags;

use cfg_if::cfg_if;

use crate::arch::{CPUOps, CPU};
use crate::cell::LazyReadOnlyCell;
use crate::env;
use crate::kif::Perm;
use crate::mem::{self, GlobOff, PhysAddr, VirtAddr};
use crate::{cfg, kif};

use crate::tcu::{
    ActId, EpId, EpType, GenId, Label, Reg, TileId, EP_REGS, EXT_REGS, MMIO_ADDR, MMIO_EPS_ADDR,
    NO_REPLIES, PRINT_REGS, TCU, UNPRIV_REGS,
};

use super::{ConfigReg, CONFIG_OFF};

/// The external commands
#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
#[repr(u64)]
pub enum ExtCmdOpCode {
    /// The idle command has no effect
    Idle,
    /// Invalidate and endpoint, if possible
    InvEP,
    /// Reset the CU
    Reset,
}

cfg_if! {
    if #[cfg(M3_TARGET = "hw22")] {
        /// The external registers
        #[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
        #[repr(u64)]
        pub enum ExtReg {
            /// Stores the privileged flag (for now)
            Features,
            /// For external commands
            ExtCmd,
        }
    }
    else if #[cfg(M3_TARGET = "hw23")] {
        /// The external registers
        #[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
        #[repr(u64)]
        pub enum ExtReg {
            /// Stores the privileged flag (for now)
            Features,
            /// Stores the tile description
            TileDesc,
            /// For external commands
            ExtCmd,
        }
    }
    else {
        /// The external registers
        #[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
        #[repr(u64)]
        pub enum ExtReg {
            /// Stores the privileged flag (for now)
            Features,
            /// Stores the tile description
            TileDesc,
            /// For external commands
            ExtCmd,
            /// Extra argument for external commands
            ExtArg1,
            /// The global address of the EP region
            EpsAddr,
            /// The size of the EP region in bytes
            EpsSize,
            /// The exclusive-region manager
            ExRegMng,
        }
    }
}

bitflags! {
    /// The status flag for the [`ExtReg::Features`] register
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct FeatureFlags : Reg {
        /// Whether the tile is privileged
        const PRIV          = 1 << 0;
        /// Whether the tile is currently locked (for a TEE)
        const LOCKED        = 1 << 3;
    }
}

static TILE_IDS: LazyReadOnlyCell<[u16; cfg::MAX_TILES * cfg::MAX_CHIPS]> =
    LazyReadOnlyCell::default();

impl TCU {
    #[cold]
    fn init_tileid_translation() {
        let mut ids = [0u16; cfg::MAX_TILES * cfg::MAX_CHIPS];

        let mut log_chip = 0;
        let mut log_tile = 0;
        let mut phys_chip = None;
        assert!(env::boot().raw_tile_count > 0);
        for id in &env::boot().raw_tile_ids[0..env::boot().raw_tile_count as usize] {
            let tid = TileId::new_from_raw(*id as u16);

            if phys_chip.is_some() {
                if phys_chip.unwrap() != tid.chip() {
                    phys_chip = Some(tid.chip());
                    log_chip += 1;
                    log_tile = 0;
                }
                else {
                    log_tile += 1;
                }
            }
            else {
                phys_chip = Some(tid.chip());
            }

            ids[log_chip * cfg::MAX_TILES + log_tile] = tid.raw();
        }

        TILE_IDS.set(ids);
    }

    #[inline]
    pub fn tileid_to_nocid(tile: TileId) -> u16 {
        if !TILE_IDS.is_some() {
            Self::init_tileid_translation();
        }

        TILE_IDS.get()[tile.chip() as usize * cfg::MAX_TILES + tile.tile() as usize]
    }

    #[inline]
    pub fn nocid_to_tileid(tile: u16) -> TileId {
        if !TILE_IDS.is_some() {
            Self::init_tileid_translation();
        }

        for (i, id) in TILE_IDS.get().iter().enumerate() {
            if *id == tile {
                let chip = i / cfg::MAX_TILES;
                let tile = i % cfg::MAX_TILES;
                return TileId::new(chip as u8, tile as u8);
            }
        }
        unreachable!();
    }

    pub fn config_recv(
        regs: &mut [Reg],
        act: ActId,
        buf: PhysAddr,
        buf_ord: u32,
        msg_ord: u32,
        reply_eps: Option<EpId>,
    ) {
        match env!("M3_TARGET") {
            "hw22" | "hw23" => {
                regs[0] = (EpType::Receive as Reg)
                    | ((act as Reg) << 3)
                    | ((reply_eps.unwrap_or(NO_REPLIES) as Reg) << 19)
                    | (((buf_ord - msg_ord) as Reg) << 35)
                    | ((msg_ord as Reg) << 41);
                regs[1] = buf.as_raw() as Reg;
                regs[2] = 0;
            },
            _ => {
                regs[0] = (EpType::Receive as Reg)
                    | ((act as Reg) << 3)
                    | ((reply_eps.unwrap_or(NO_REPLIES) as Reg) << 19)
                    | (((buf_ord - msg_ord) as Reg) << 35)
                    | ((msg_ord as Reg) << 42);
                regs[1] = buf.as_raw() as Reg;
                regs[2] = 0;
                regs[3] = 0;
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn config_send(
        regs: &mut [Reg],
        act: ActId,
        lbl: Label,
        tile: TileId,
        _gen: GenId,
        dst_ep: EpId,
        msg_order: u32,
        credits: u32,
    ) {
        match env!("M3_TARGET") {
            "hw22" | "hw23" => {
                regs[0] = (EpType::Send as Reg)
                    | ((act as Reg) << 3)
                    | ((credits as Reg) << 19)
                    | ((credits as Reg) << 25)
                    | ((msg_order as Reg) << 31);
                regs[1] = (dst_ep as Reg) | ((Self::tileid_to_nocid(tile) as Reg) << 16);
                regs[2] = lbl as Reg;
            },
            _ => {
                regs[0] = (EpType::Send as Reg)
                    | ((act as Reg) << 3)
                    | ((credits as Reg) << 19)
                    | ((credits as Reg) << 26)
                    | ((msg_order as Reg) << 33);
                regs[1] = (dst_ep as Reg)
                    | ((Self::tileid_to_nocid(tile) as Reg) << 16)
                    | ((_gen as Reg) << 30);
                regs[2] = lbl as Reg;
                regs[3] = 0;
            },
        }
    }

    pub fn config_mem(
        regs: &mut [Reg],
        act: ActId,
        tile: TileId,
        gen: GenId,
        addr: GlobOff,
        size: usize,
        perm: Perm,
    ) {
        Self::config_mem_raw(
            regs,
            act,
            Self::tileid_to_nocid(tile),
            gen,
            addr,
            size,
            perm,
        )
    }

    pub fn config_mem_raw(
        regs: &mut [Reg],
        act: ActId,
        tile_noc_id: u16,
        gen: GenId,
        addr: GlobOff,
        size: usize,
        perm: Perm,
    ) {
        regs[0] = (EpType::Memory as Reg)
            | ((act as Reg) << 3)
            | ((perm.bits() as Reg) << 19)
            | ((tile_noc_id as Reg) << 23)
            | ((gen as Reg) << 37);
        regs[1] = addr as Reg;
        regs[2] = size as Reg;
        if env!("M3_TARGET") == "gem5" || env!("M3_TARGET") == "hw" {
            regs[3] = 0;
        }
    }

    pub fn config_invalid(regs: &mut [Reg], act: ActId, dynamic: bool) {
        regs[1] = 0;
        regs[2] = 0;
        if env!("M3_TARGET") == "gem5" || env!("M3_TARGET") == "hw" {
            regs[3] = 0;
        }
        let dyn_flag = if dynamic { 1 << 62 } else { 0 };
        regs[0] = (EpType::Invalid as Reg) | ((act as Reg) << 3) | dyn_flag;
    }

    /// Configures the given endpoint
    pub fn set_ep_regs(ep: EpId, regs: &[Reg]) {
        let off = EP_REGS * ep as usize;
        unsafe {
            let addr = (MMIO_EPS_ADDR.as_mut_ptr::<Reg>()).add(off);
            // write r0 last because that might freeze the EP
            for (i, r) in regs.iter().enumerate().rev() {
                CPU::write8b(addr.add(i), *r);
            }
        }
        // ensure that all accesses are finished before we try to use the EP
        CPU::memory_barrier();
    }

    #[allow(unused, clippy::too_many_arguments)]
    pub fn build_exreg(
        user_tile: TileId,
        user_tile_gen: GenId,
        idx: usize,
        addr: GlobOff,
        size: GlobOff,
        perm: kif::Perm,
    ) -> Option<(Reg, Reg)> {
        #[cfg(not(any(M3_TARGET = "hw", M3_TARGET = "gem5")))]
        return None;

        #[cfg(any(M3_TARGET = "hw", M3_TARGET = "gem5"))]
        {
            let mut cfg = (user_tile_gen as Reg) << 16 | (user_tile.raw() as Reg) << 2;
            if perm.contains(kif::Perm::R) {
                cfg |= 1 << 0;
            }
            if perm.contains(kif::Perm::W) {
                cfg |= 1 << 1;
            }

            assert!(((addr >> 3) & ((size >> 3) - 1)) == 0);
            assert!(size.is_power_of_two());
            let addr_size = (addr >> 2) | ((size >> 3) - 1);
            Some((cfg, addr_size))
        }
    }

    /// Returns the value for the `ExtCmd` register for given opcode and argument.
    pub fn build_ext_cmd(cmd: ExtCmdOpCode, arg: u64) -> Reg {
        match env!("M3_TARGET") {
            "hw22" | "hw23" => (cmd as Reg) | (arg << 9),
            _ => (cmd as Reg) | (arg << 10),
        }
    }

    /// Returns the MMIO address for the given external register
    pub fn ext_reg_addr(reg: ExtReg) -> VirtAddr {
        MMIO_ADDR + (reg as usize) * mem::size_of::<Reg>()
    }

    /// Returns the MMIO address of the given endpoint registers
    pub fn ep_regs_addr(ep: EpId) -> VirtAddr {
        MMIO_EPS_ADDR + (EP_REGS * ep as usize) * mem::size_of::<Reg>()
    }

    /// Returns the MMIO address of the given exclusive register
    pub fn exreg_addr(reg: usize) -> VirtAddr {
        MMIO_ADDR + (EXT_REGS + UNPRIV_REGS + PRINT_REGS + reg * 2) * mem::size_of::<Reg>()
    }

    /// Returns the MMIO address of the given config register
    pub fn config_addr(reg: ConfigReg) -> VirtAddr {
        MMIO_ADDR + CONFIG_OFF + (reg as usize) * mem::size_of::<Reg>()
    }
}
