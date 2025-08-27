/*
 * Copyright (C) 2025 Nils Asmussen, Barkhausen Institut
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

use base::{io::LogFlags, log, time::CycleInstant};

use num_enum::{FromPrimitive, IntoPrimitive};

// TODO verify that
const COMP_TIME: u64 = 22;

type Reg = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromPrimitive, IntoPrimitive)]
#[repr(usize)]
enum AESReg {
    #[default]
    Enable     = 0,
    KeyAddr    = 5,
    InputAddr  = 6,
    OutputAddr = 7,
}

/// A simple wrapper around AES accelerator on the hardware platform.
pub struct AESAcc;

impl Default for AESAcc {
    fn default() -> Self {
        let acc = AESAcc;
        // TODO set the encryption key
        acc.write_reg(AESReg::KeyAddr, 0);
        acc
    }
}

impl AESAcc {
    pub fn encrypt(&self, in_off: usize, out_off: usize) {
        log!(
            LogFlags::AESCmd,
            "AES::encrypt(in={}, out={})",
            in_off,
            out_off
        );
        // TODO this does currently not work, i.e., the accelerator does not put the encrypted data
        // at `out_off`. I don't know why yet.
        self.write_reg(AESReg::InputAddr, in_off as Reg);
        self.write_reg(AESReg::OutputAddr, out_off as Reg);
        // TODO this is currently already enabled on tile reset. Not sure if that's a real problem,
        // but we should fix that somehow :)
        self.write_reg(AESReg::Enable, 1);
        let end = CycleInstant::now().as_cycles() + COMP_TIME;
        while CycleInstant::now().as_cycles() < end {}
        self.write_reg(AESReg::Enable, 0);
    }

    fn write_reg(&self, reg: AESReg, value: Reg) {
        log!(
            LogFlags::AESReg,
            "AES::write_reg({:?}) <- {:#x}",
            reg,
            value
        );
        #[cfg(any(M3_TARGET = "hw23", M3_TARGET = "hw"))]
        {
            use base::cpu::{CPUOps, CPU};
            use core::mem::size_of;
            const MMIO_ADDR: usize = 0xF0003030;

            let addr = MMIO_ADDR + (reg as usize) * size_of::<Reg>();
            unsafe {
                CPU::write8b(addr as *mut Reg, value);
            }
        }
    }
}
