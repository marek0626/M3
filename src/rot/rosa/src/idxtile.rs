/*
 * Copyright (C) 2025 Nils Asmussen, Barkhausen Institut
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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

use base::errors::{Code, Error};
use base::kif::Perm;
use base::mem::GlobOff;
use base::tcu;
use base::tcu::{TileId, TCU};

use crate::{config_local_ep, EP_REGS_SIZE};

pub const TILE_TCU_EP_START: tcu::EpId = 32;

/// A tile indexed relative to its order in `env::boot().raw_tile_ids`
///
/// The idea is to make each TCU's MMIO area available at a specific range of EPs. This is required
/// at setup time to read and write to TCUs (e.g., read tile descriptions, reset kernel TCU, etc.),
/// and is required later to read out the state of every TCU in the system.
#[derive(Copy, Clone, Debug)]
pub struct IndexedTile {
    id: TileId,
    idx: usize,
}

impl IndexedTile {
    pub fn new(id: TileId, idx: usize) -> Self {
        Self { id, idx }
    }

    pub fn id(&self) -> TileId {
        self.id
    }

    pub fn index(&self) -> usize {
        self.idx
    }

    fn ep(&self) -> tcu::EpId {
        TILE_TCU_EP_START + self.idx as tcu::EpId
    }

    pub fn init(&self, perm: Perm) {
        config_local_ep(self.ep(), |regs| {
            TCU::config_mem(
                regs,
                rot::TCU_ACT_ID,
                self.id,
                0,
                tcu::MMIO_ADDR.as_goff(),
                tcu::MMIO_SIZE + tcu::MMIO_PRIV_SIZE + (1 << 16) * EP_REGS_SIZE,
                perm,
            );
        });
    }

    #[cfg(feature = "gem5")]
    pub fn config_ep<CFG>(&self, ep: tcu::EpId, cfg: CFG)
    where
        CFG: FnOnce(&mut [tcu::Reg]),
    {
        let mut regs = [0; tcu::EP_REGS];
        cfg(&mut regs);
        self.write_tcu(&regs[..], TCU::ep_regs_addr(ep).as_goff())
            .expect("Failed to configure remote TCU endpoint");
    }

    pub fn read_tcu_obj<T>(&self, off: GlobOff) -> Result<T, Error> {
        TCU::read_obj(self.ep(), off - tcu::MMIO_ADDR.as_goff())
    }

    pub fn write_tcu<T>(&self, regs: &[T], off: GlobOff) -> Result<(), Error> {
        TCU::write_slice(self.ep(), regs, off - tcu::MMIO_ADDR.as_goff())
    }

    pub fn ext_cmd(&self, cmd: tcu::Reg) -> Result<tcu::Reg, Error> {
        let addr = TCU::ext_reg_addr(tcu::ExtReg::ExtCmd).as_goff();
        self.write_tcu(&[cmd], addr)?;
        self.wait_ext_cmd()
    }

    fn wait_ext_cmd(&self) -> Result<tcu::Reg, Error> {
        let addr = TCU::ext_reg_addr(tcu::ExtReg::ExtCmd).as_goff();

        let res = loop {
            let res: tcu::Reg = self.read_tcu_obj(addr)?;
            let idle_code: tcu::Reg = tcu::ExtCmdOpCode::Idle.into();
            if (res & 0xF) == idle_code {
                break res;
            }
        };

        match Code::try_from(((res >> 4) & 0x3F) as u32).unwrap() {
            Code::Success => Ok(res >> 10),
            e => Err(Error::new(e)),
        }
    }
}
