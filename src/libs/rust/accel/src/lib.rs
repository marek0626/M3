/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2020 Nils Asmussen, Barkhausen Institut
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

#![no_std]

#[allow(unused_extern_crates)]
extern crate lang;

use m3core::com::{EpMng, MemCap, MemGate, RecvCap, RecvGate, SGateArgs, SendCap, SendGate, EP};
use m3core::errors::{Code, Error};
use m3core::kif::Perm;
use m3core::mem::{GlobOff, VirtAddr, VirtAddrRaw};
use m3core::rc::Rc;
use m3core::tcu::{EpId, Label};
use m3core::tiles::{ChildActivity, Tile};
use m3core::util::math::next_log2;
use m3core::vfs::{File, FileRef};

const MSG_SIZE: usize = 64;
const RB_SIZE: usize = MSG_SIZE * 4;
const BUF_ADDR: GlobOff = 0x8000;
const BUF_SIZE: GlobOff = 8192;

const EP_IN_SEND: EpId = 16;
const EP_IN_MEM: EpId = 17;
const EP_OUT_SEND: EpId = 18;
const EP_OUT_MEM: EpId = 19;
const EP_RECV: EpId = 20;

const LBL_IN_REQ: Label = 1;
const LBL_OUT_REQ: Label = 3;

pub struct StreamAccel {
    tile: Rc<Tile>,
    _rgate: RecvGate,
    in_sep: Option<EP>,
    in_mep: EP,
    out_sep: Option<EP>,
    out_mep: Option<EP>,
    mem: MemCap,
    sgate_in: Option<SendGate>,
    sgate_out: Option<SendGate>,
    mgate_out: Option<MemGate>,
    tee: bool,
}

impl StreamAccel {
    pub fn new(act: &ChildActivity, tee: bool) -> Result<Self, Error> {
        let rcap = RecvCap::new(next_log2(RB_SIZE), next_log2(MSG_SIZE))?;

        let in_sep = Some(EpMng::acquire_for(act.sel(), EP_IN_SEND, 0, false)?);
        let in_mep = EpMng::acquire_for(act.sel(), EP_IN_MEM, 0, tee)?;

        let out_sep = Some(EpMng::acquire_for(act.sel(), EP_OUT_SEND, 0, false)?);
        let out_mep = Some(EpMng::acquire_for(act.sel(), EP_OUT_MEM, 0, tee)?);

        let rep = EpMng::acquire_for(act.sel(), EP_RECV, RB_SIZE / MSG_SIZE, false)?;
        let recv_addr = VirtAddr::new(act.tile_desc().mem_offset() as VirtAddrRaw + 0x1_4C00);
        let _rgate = rcap.activate_with(None, recv_addr.as_goff(), recv_addr, Some(rep))?;

        let mem = MemCap::new_foreign(
            act.sel(),
            VirtAddr::from(act.tile_desc().mem_offset()),
            act.tile_desc().mem_size() as GlobOff,
            Perm::RW,
        )?;

        Ok(Self {
            tile: act.tile().clone(),
            _rgate,
            in_sep,
            in_mep,
            out_sep,
            out_mep,
            mem,
            sgate_in: None,
            sgate_out: None,
            mgate_out: None,
            tee,
        })
    }

    pub fn attach_input(&mut self, file: &mut FileRef<dyn File>) -> Result<(), Error> {
        file.attach(
            self.in_sep.take().ok_or_else(|| Error::new(Code::Exists))?,
            &self.in_mep,
        )
    }

    pub fn attach_input_accel(&mut self, prev: &StreamAccel) -> Result<(), Error> {
        let scap_in = SendCap::new_with(SGateArgs::new(&prev._rgate).label(LBL_IN_REQ).credits(1))?;
        let sgate_in =
            scap_in.activate_on(self.in_sep.take().ok_or_else(|| Error::new(Code::Exists))?)?;
        self.sgate_in = Some(sgate_in);
        Ok(())
    }

    pub fn attach_output(&mut self, file: &mut FileRef<dyn File>) -> Result<(), Error> {
        file.attach(
            self.out_sep
                .take()
                .ok_or_else(|| Error::new(Code::Exists))?,
            self.out_mep.as_ref().unwrap(),
        )
    }

    pub fn attach_output_accel(&mut self, next: &StreamAccel) -> Result<(), Error> {
        let scap_out =
            SendCap::new_with(SGateArgs::new(&next._rgate).label(LBL_OUT_REQ).credits(1))?;
        let sgate_out = scap_out.activate_on(
            self.out_sep
                .take()
                .ok_or_else(|| Error::new(Code::Exists))?,
        )?;
        self.sgate_out = Some(sgate_out);

        let mcap_out = next.mem.derive(BUF_ADDR, BUF_SIZE, Perm::RW)?;
        if self.tee {
            mcap_out.make_exclusive(&next.tile, &self.tile, true)?;
        }
        let mgate_out = mcap_out.activate_on(
            self.out_mep
                .take()
                .ok_or_else(|| Error::new(Code::Exists))?,
        )?;
        self.mgate_out = Some(mgate_out);
        Ok(())
    }
}
