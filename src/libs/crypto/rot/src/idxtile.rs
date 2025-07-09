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

use base::env;
use base::errors::{Code, Error};
use base::kif::Perm;
use base::mem::GlobOff;
use base::tcu::{self, TileId, TCU};

pub const TILE_TCU_EP_START: tcu::EpId = 32;

pub fn config_local_ep<CFG>(ep: tcu::EpId, cfg: CFG)
where
    CFG: FnOnce(&mut [tcu::Reg]),
{
    let mut regs = [0; tcu::EP_REGS];
    cfg(&mut regs);
    TCU::set_ep_regs(ep, &regs);
}

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

    pub fn new_from_env(id: TileId) -> Option<Self> {
        let idx = env::boot().raw_tile_ids[0..env::boot().raw_tile_count as usize]
            .iter()
            .position(|raw_id| TCU::nocid_to_tileid(*raw_id as u16) == id)?;
        Some(Self::new(id, idx))
    }

    pub fn id(&self) -> TileId {
        self.id
    }

    pub fn index(&self) -> usize {
        self.idx
    }

    pub fn ep(&self) -> tcu::EpId {
        TILE_TCU_EP_START + self.idx as tcu::EpId
    }

    pub fn init(&self, perm: Perm, gen: tcu::GenId) {
        config_local_ep(self.ep(), |regs| {
            TCU::config_mem(regs, crate::TCU_ACT_ID, self.id, gen, 0, 0xFFFF_FFFF, perm);
        });
    }

    pub fn config_ep<CFG>(&self, ep: tcu::EpId, cfg: CFG)
    where
        CFG: FnOnce(&mut [tcu::Reg]),
    {
        let mut regs = [0; tcu::EP_REGS];
        cfg(&mut regs);

        if env!("M3_TARGET") == "gem5" {
            self.write_tcu(&regs[..], TCU::ep_regs_addr(ep).as_goff())
                .expect("Failed to configure remote TCU endpoint");
        }
        else {
            for (i, r) in regs.iter().enumerate() {
                self.write_tcu(&[*r], (TCU::ep_regs_addr(ep) + i * 8).as_goff())
                    .expect("Failed to configure remote TCU endpoint");
            }
        }
    }

    pub fn invalidate_ep(&self, ep: tcu::EpId) -> Result<(), Error> {
        let reg = TCU::build_ext_cmd(tcu::ExtCmdOpCode::InvEP, (ep as u64) | 1_u64 << 16);
        self.ext_cmd(reg).map(|_| ())
    }

    pub fn read_tcu_obj<T>(&self, off: GlobOff) -> Result<T, Error> {
        TCU::read_obj(self.ep(), off)
    }

    pub fn write_tcu<T>(&self, regs: &[T], off: GlobOff) -> Result<(), Error> {
        TCU::write_slice(self.ep(), regs, off)
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
