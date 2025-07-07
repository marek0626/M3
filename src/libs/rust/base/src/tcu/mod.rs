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

//! The Trusted Communication Unit interface

mod ext;
mod msg;
mod r#priv;
mod unpriv;

pub use ext::*;
pub use msg::*;
pub use r#priv::*;
pub use unpriv::*;

use core::fmt;

use cfg_if::cfg_if;

use num_enum::IntoPrimitive;

use serde::{Deserialize, Serialize};

use crate::{
    cfg,
    cpu::{CPUOps, CPU},
    kif::PageFlags,
    mem::{self, VirtAddr, VirtAddrRaw},
};

/// A TCU register
pub type Reg = u64;
/// An endpoint id
pub type EpId = u16;
/// A TCU label used in send EPs
#[cfg(M3_TARGET = "hw22")]
pub type Label = u32;
#[cfg(not(M3_TARGET = "hw22"))]
pub type Label = u64;
/// A activity id
pub type ActId = u16;
/// A tile-generation id
pub type GenId = u16;

#[cfg(M3_TARGET = "gem5")]
pub const EXREG_REGS: usize = 16;
#[cfg(not(M3_TARGET = "gem5"))]
pub const EXREG_REGS: usize = 0;
pub const PMEM_PROT_EPS: usize = 4;
pub const TILEMUX_EPS: usize = 4;

/// The send EP for kernel calls from TileMux
pub const KPEX_SEP: EpId = PMEM_PROT_EPS as EpId + 0;
/// The receive EP for kernel calls from TileMux
pub const KPEX_REP: EpId = PMEM_PROT_EPS as EpId + 1;
/// The receive EP for sidecalls from the kernel for TileMux
pub const TMSIDE_REP: EpId = PMEM_PROT_EPS as EpId + 2;
/// The reply EP for sidecalls from the kernel for TileMux
pub const TMSIDE_RPLEP: EpId = PMEM_PROT_EPS as EpId + 3;

/// The send EP offset for system calls
pub const SYSC_SEP_OFF: EpId = 0;
/// The receive EP offset for system calls
pub const SYSC_REP_OFF: EpId = 1;
/// The receive EP offset for upcalls from the kernel
pub const UPCALL_REP_OFF: EpId = 2;
/// The reply EP offset for upcalls from the kernel
pub const UPCALL_RPLEP_OFF: EpId = 3;
/// The default receive EP offset
pub const DEF_REP_OFF: EpId = 4;
/// The pager send EP offset
pub const PG_SEP_OFF: EpId = 5;
/// The pager receive EP offset
pub const PG_REP_OFF: EpId = 6;

/// The offset of the first user EP
pub const FIRST_USER_EP: EpId = PMEM_PROT_EPS as EpId + TILEMUX_EPS as EpId;
/// The number of standard EPs
pub const STD_EPS_COUNT: usize = 7;

/// An invalid endpoint ID
pub const INVALID_EP: EpId = 0xFFFF;
/// The reply EP for messages that want to disable replies
pub const NO_REPLIES: EpId = INVALID_EP;

/// The base address of the TCU's MMIO area
pub const MMIO_ADDR: VirtAddr = VirtAddr::new(0xF000_0000);
/// The size of the TCU's privileged MMIO area
pub const MMIO_PRIV_SIZE: usize = cfg::PAGE_SIZE;

/// The number of PRINT registers
pub const PRINT_REGS: usize = 32;
cfg_if! {
    if #[cfg(M3_TARGET = "hw22")] {
        /// The number of external registers
        pub const EXT_REGS: usize = 2;
        /// The number of unprivileged registers
        pub const UNPRIV_REGS: usize = 5;
    }
    else if #[cfg(M3_TARGET = "hw23")] {
        /// The number of external registers
        pub const EXT_REGS: usize = 3;
        /// The number of unprivileged registers
        pub const UNPRIV_REGS: usize = 6;
    }
    else {
        /// The number of external registers
        pub const EXT_REGS: usize = 7;
        /// The number of unprivileged registers
        pub const UNPRIV_REGS: usize = 6;
    }
}
cfg_if! {
    if #[cfg(any(M3_TARGET = "hw22", M3_TARGET = "hw23"))] {
        /// The number of registers per EP
        pub const EP_REGS: usize = 3;

        /// Represents unlimited credits for send EPs
        pub const UNLIM_CREDITS: u32 = 0x3F;

        /// The size of the TCU's MMIO area
        pub const MMIO_SIZE: usize = cfg::PAGE_SIZE * 2;
        /// The base address of the TCU's privileged MMIO area
        pub const MMIO_PRIV_ADDR: VirtAddr =
            VirtAddr::new(MMIO_ADDR.as_raw() + (cfg::PAGE_SIZE * 2) as VirtAddrRaw);
        /// The base address of the TCU's endpoint MMIO area
        pub const MMIO_EPS_ADDR: VirtAddr = VirtAddr::new(
            MMIO_ADDR.as_raw() +
                ((EXT_REGS + UNPRIV_REGS) * mem::size_of::<Reg>()) as VirtAddrRaw
        );
    }
    else {
        /// The number of registers per EP
        pub const EP_REGS: usize = 4;

        /// Represents unlimited credits for send EPs
        pub const UNLIM_CREDITS: u32 = 0x7F;

        /// The size of the TCU's MMIO area
        pub const MMIO_SIZE: usize = cfg::PAGE_SIZE;
        /// The base address of the TCU's privileged MMIO area
        pub const MMIO_PRIV_ADDR: VirtAddr =
            VirtAddr::new(MMIO_ADDR.as_raw() + MMIO_SIZE as VirtAddrRaw);
        /// The base address of the TCU's endpoint MMIO area
        pub const MMIO_EPS_ADDR: VirtAddr =
            VirtAddr::new(MMIO_PRIV_ADDR.as_raw() + MMIO_PRIV_SIZE as VirtAddrRaw);
    }
}

/// A tile id, consisting of a chip and chip-local tile id
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileId {
    id: u16,
}

impl TileId {
    /// Constructs a new tile id out of the given chip and chip-local tile id
    pub const fn new(chip: u8, tile: u8) -> Self {
        Self {
            id: (chip as u16) << 8 | tile as u16,
        }
    }

    /// Constructs a new tile id from the given raw id (e.g., as stored in TCUs)
    pub const fn new_from_raw(raw: u16) -> Self {
        Self { id: raw }
    }

    /// Returns the chip id
    pub const fn chip(&self) -> u8 {
        (self.id >> 8) as u8
    }

    /// Returns the chip-local tile id
    pub const fn tile(&self) -> u8 {
        (self.id & 0xFF) as u8
    }

    /// Returns the raw representation of the id (e.g., as stored in TCUs)
    pub const fn raw(&self) -> u16 {
        self.id
    }
}

impl fmt::Display for TileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "C{}T{:02}", self.chip(), self.tile())
    }
}

/// The different endpoint types
#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive)]
#[repr(u64)]
pub enum EpType {
    /// Invalid endpoint (unusable)
    Invalid,
    /// Send endpoint
    Send,
    /// Receive endpoint
    Receive,
    /// Memory endpoint
    Memory,
}

/// The TCU interface
pub struct TCU {}

impl TCU {
    /// Returns all MMIO areas that need to be mapped
    pub fn mmio_areas() -> [(VirtAddr, usize, PageFlags); 3] {
        match env!("M3_TARGET") {
            "hw22" | "hw23" => [
                (MMIO_ADDR, cfg::PAGE_SIZE * 2, PageFlags::U | PageFlags::RW),
                (
                    MMIO_PRIV_ADDR,
                    cfg::PAGE_SIZE * 2,
                    PageFlags::U | PageFlags::RW,
                ),
                (VirtAddr::null(), 0, PageFlags::empty()),
            ],
            _ => [
                (MMIO_ADDR, MMIO_SIZE, PageFlags::U | PageFlags::RW),
                (MMIO_PRIV_ADDR, MMIO_PRIV_SIZE, PageFlags::U | PageFlags::RW),
                (
                    MMIO_EPS_ADDR,
                    Self::endpoints_size(),
                    PageFlags::U | PageFlags::R,
                ),
            ],
        }
    }

    /// Returns the size of the endpoints region (according to the EPS_SIZE register)
    pub fn endpoints_size() -> usize {
        #[cfg(any(M3_TARGET = "hw22", M3_TARGET = "hw23"))]
        return 128 * EP_REGS * mem::size_of::<Reg>();
        #[cfg(not(any(M3_TARGET = "hw22", M3_TARGET = "hw23")))]
        return Self::read_reg(ExtReg::EpsSize as usize) as usize;
    }

    /// Returns true if the TCU is locked
    ///
    /// In the locked state, the TCU's endpoints cannot be changed anymore without agreement of the
    /// application that owns the endpoints. If the kernel changes a non-dynamic EP, the EP is
    /// frozen and needs to be unfreezed by the application before being usable. Dynamic EPs can be
    /// changed by the kernel without agreement though, but these need to marked dynamic by the
    /// application beforehand.
    ///
    /// Furthermore, in the locked state all incoming memory requests (reads/writes to the tile
    /// internal memory are denied.
    pub fn is_locked() -> bool {
        let features = Self::read_reg(ExtReg::Features as usize);
        (features & FeatureFlags::LOCKED.bits()) != 0
    }

    /// Writes the given address and size into the Data register
    pub fn write_data(addr: VirtAddr, size: usize) {
        #[cfg(M3_TARGET = "hw22")]
        Self::write_unpriv_reg(
            UnprivReg::Data,
            (size as Reg) << 32 | addr.as_local() as Reg,
        );
        #[cfg(not(M3_TARGET = "hw22"))]
        {
            Self::write_unpriv_reg(UnprivReg::DataAddr, addr.as_local() as Reg);
            Self::write_unpriv_reg(UnprivReg::DataSize, size as Reg);
        }
    }

    /// Returns the contents of the Data register (address and size)
    pub fn read_data() -> (usize, usize) {
        #[cfg(M3_TARGET = "hw22")]
        {
            let data = Self::read_unpriv_reg(UnprivReg::Data);
            ((data & 0xFFFF_FFFF) as usize, (data >> 32) as usize)
        }
        #[cfg(not(M3_TARGET = "hw22"))]
        {
            (
                Self::read_unpriv_reg(UnprivReg::DataAddr) as usize,
                Self::read_unpriv_reg(UnprivReg::DataSize) as usize,
            )
        }
    }

    /// Returns the value of the given unprivileged register
    pub fn read_unpriv_reg(reg: UnprivReg) -> Reg {
        Self::read_reg(EXT_REGS + reg as usize)
    }

    /// Sets the value of the given unprivileged register to `val`
    pub fn write_unpriv_reg(reg: UnprivReg, val: Reg) {
        Self::write_reg(EXT_REGS + reg as usize, val)
    }

    pub(crate) fn write_cfg_reg(reg: ConfigReg, val: Reg) {
        Self::write_reg(
            ((cfg::PAGE_SIZE * 3) / mem::size_of::<Reg>()) + reg as usize,
            val,
        )
    }

    pub(crate) fn read_ep_reg(ep: EpId, reg: usize) -> Reg {
        Self::read_reg(
            (MMIO_EPS_ADDR.as_local() - MMIO_ADDR.as_local()) / mem::size_of::<Reg>()
                + EP_REGS * ep as usize
                + reg,
        )
    }

    pub(crate) fn read_reg(idx: usize) -> Reg {
        // safety: we know that the address is within the MMIO region of the TCU
        unsafe { CPU::read8b((MMIO_ADDR.as_ptr::<Reg>()).add(idx)) }
    }

    pub(crate) fn write_reg(idx: usize, val: Reg) {
        // safety: as above
        unsafe { CPU::write8b((MMIO_ADDR.as_mut_ptr::<Reg>()).add(idx), val) };
    }
}
