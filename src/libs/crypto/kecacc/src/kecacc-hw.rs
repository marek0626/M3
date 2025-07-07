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

use core::mem::size_of;

use base::{
    crypto::HashType,
    env,
    io::LogFlags,
    kif::Perm,
    log,
    mem::GlobOff,
    tcu::{self, EpId, TCU},
};

use num_enum::{FromPrimitive, IntoPrimitive};

const STATE_SIZE: usize = 13 * 16;

/// Represents a saved state of the KecAcc accelerator.
#[derive(Clone)]
#[repr(align(16))]
pub struct KecAccState {
    data: [u8; STATE_SIZE],
}

impl KecAccState {
    pub const fn new() -> Self {
        Self {
            data: [0u8; STATE_SIZE],
        }
    }
}

impl Default for KecAccState {
    fn default() -> Self {
        Self::new()
    }
}

const MMIO_ADDR: usize = 0xF00030A8;
const MMIO_EP: EpId = 36;
const MEM_EP: EpId = 55;
// TODO the accelerator SPM is actually 4096 bytes large, but the accelerator can only deal with 2K
// at the moment.
const MEM_SIZE: usize = 0x800;

type Reg = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromPrimitive, IntoPrimitive)]
#[repr(usize)]
enum KeccakReg {
    #[default]
    Id = 0,
    Config1,
    Config2,
    CmdBuf,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromPrimitive, IntoPrimitive)]
#[repr(usize)]
enum CmdType {
    /// Accelerator is idle (has completed previous command)
    #[default]
    Idle = 0,
    /// Initialize accelerator with specified hash type
    Init,
    /// Absorb bytes from specified memory address
    Absorb,
    /// Squeeze bytes to specified memory address
    Squeeze,
    /// Save accelerator state to specified memory address
    Save,
    /// Load accelerator state from specified memory address
    Restore,
    Permute,
}

#[derive(Debug)]
#[allow(unused)]
struct CmdStatus {
    empty: bool,
    full: bool,
    space: u8,
    cnt: u16,
}

struct Algorithm {
    lanes_per_blk_size: Reg,
    xof_size: Reg,
}

static ALGOS: [Algorithm; 6] = [
    Algorithm {
        // SHA3-224,
        lanes_per_blk_size: 18,
        xof_size: 0,
    },
    Algorithm {
        // SHA3-256,
        lanes_per_blk_size: 17,
        xof_size: 0,
    },
    Algorithm {
        // SHA3-384,
        lanes_per_blk_size: 13,
        xof_size: 0,
    },
    Algorithm {
        // SHA3-512,
        lanes_per_blk_size: 9,
        xof_size: 0,
    },
    Algorithm {
        // SHAKE128,
        lanes_per_blk_size: 21,
        xof_size: 1,
    },
    Algorithm {
        // SHAKE256,
        lanes_per_blk_size: 17,
        xof_size: 1,
    },
];

pub fn init() {
    let mut regs = [0; tcu::EP_REGS];
    TCU::config_mem(
        &mut regs,
        0,
        env::boot().tile_id(),
        0,
        0x20_0000 as GlobOff,
        0x1000,
        Perm::RW,
    );
    TCU::set_ep_regs(MEM_EP, &regs);

    TCU::config_mem(
        &mut regs,
        0,
        env::boot().tile_id(),
        0,
        0,
        0xFFFF_FFFF,
        Perm::RW,
    );
    TCU::set_ep_regs(MMIO_EP, &regs);
}

/// A simple wrapper around the Keccak/SHA-3 accelerator on the hardware platform.
///
/// NOTE: all functions work synchronously despite their name, because the accelerator always
/// writes to his private SPM so that we have do perform copy tasks from that SPM to the actual
/// location in our memory *after* the command has finished. For simplicity, we therefore just wait
/// until the command is finished.
pub struct KecAcc {
    addr: usize,
}

impl KecAcc {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        KecAcc { addr: MMIO_ADDR }
    }

    pub fn supports_algo(&self, hash_type: HashType) -> bool {
        matches!(
            hash_type,
            HashType::SHA3_224
                | HashType::SHA3_256
                | HashType::SHA3_384
                | HashType::SHA3_512
                | HashType::SHAKE128
                | HashType::SHAKE256
        )
    }

    pub fn is_busy(&self) -> bool {
        // not async; never busy
        false
    }

    pub fn poll_complete(&self) {
        // not async; nothing to do
    }

    pub fn poll_complete_barrier(&self) {
        // not async; nothing to do
    }

    fn exec_cmd(&self, cmd: Reg) {
        let status = self.cmd_status();
        let org_cnt = status.cnt;
        self.write_reg(KeccakReg::CmdBuf, cmd);

        loop {
            let status = self.cmd_status();
            let error = self.read_reg(KeccakReg::Error);
            assert_eq!(error, 0, "Command {:#x} finished with error {}", cmd, error);
            if org_cnt != status.cnt {
                break;
            }
        }
    }

    pub fn start_init(&self, hash_type: HashType) {
        log!(LogFlags::SHA3Cmd, "SHA3::init({:?})", hash_type);
        match hash_type {
            HashType::NONE => self.write_reg(KeccakReg::Config1, 0),
            hash_type => {
                self.write_reg(KeccakReg::Config1, 1);

                let algo = &ALGOS[(hash_type as usize) - 1];
                let cmd =
                    (CmdType::Init as Reg) | (algo.lanes_per_blk_size << 4) | (algo.xof_size << 9);
                self.exec_cmd(cmd);
            },
        }
    }

    pub fn start_load(&self, state: &KecAccState) {
        log!(LogFlags::SHA3Cmd, "SHA3::load()");
        let off = 0;
        self.write_mem(&state.data[..], off);

        let cmd = (CmdType::Restore as Reg) | ((off as Reg) << 4);
        self.exec_cmd(cmd);
    }

    pub fn start_save(&self, state: &mut KecAccState) {
        log!(LogFlags::SHA3Cmd, "SHA3::save()");
        let off = 0;
        let cmd = (CmdType::Save as Reg) | ((off as Reg) << 4);
        self.exec_cmd(cmd);

        self.read_mem(&mut state.data[..], off);
    }

    pub fn start_absorb(&self, buf: &[u8]) {
        log!(LogFlags::SHA3Cmd, "SHA3::absorb({}b)", buf.len());
        self.do_absorb(buf, false);
    }

    pub fn start_pad(&self) {
        // nothing to do
    }

    pub fn start_absorb_last(&self, buf: &[u8]) {
        log!(LogFlags::SHA3Cmd, "SHA3::absorb_last({}b)", buf.len());
        self.do_absorb(buf, true);
    }

    pub fn start_squeeze(&self, mut buf: &mut [u8]) {
        log!(LogFlags::SHA3Cmd, "SHA3::squeeze({}b)", buf.len());

        while !buf.is_empty() {
            let off = 0;

            let amount = buf.len().min(MEM_SIZE);
            let cmd = (CmdType::Squeeze as Reg) | ((off as Reg) << 4) | ((amount as Reg) << 34);
            self.exec_cmd(cmd);

            self.read_mem(&mut buf[0..amount], off);

            buf = &mut buf[amount..];
        }
    }

    fn do_absorb(&self, mut buf: &[u8], finish: bool) {
        while !buf.is_empty() {
            let off = 0;

            let amount = buf.len().min(MEM_SIZE);
            self.write_mem(&buf[0..amount], off);

            let last = amount == buf.len() && finish;
            let cmd = (CmdType::Absorb as Reg)
                | ((off as Reg) << 4)
                | ((amount as Reg) << 34)
                | ((last as Reg) << 46);
            self.exec_cmd(cmd);

            buf = &buf[amount..];
        }
    }

    fn cmd_status(&self) -> CmdStatus {
        let cmd = self.read_reg(KeccakReg::CmdBuf);
        CmdStatus {
            empty: (cmd & 0x1) != 0,
            full: (cmd & 0x2) != 0,
            space: ((cmd >> 16) & 0xF) as u8,
            cnt: ((cmd >> 32) & 0xFFFF) as u16,
        }
    }

    fn read_mem(&self, dest: &mut [u8], off: usize) {
        log!(
            LogFlags::SHA3Mem,
            "SHA3::read_mem({:?}:{:?})",
            dest.as_ptr(),
            dest.len()
        );
        TCU::read(MEM_EP, dest.as_mut_ptr(), dest.len(), off as GlobOff).unwrap();
    }

    fn write_mem(&self, src: &[u8], off: usize) {
        log!(
            LogFlags::SHA3Mem,
            "SHA3::write_mem({:?}:{:?})",
            src.as_ptr(),
            src.len()
        );
        TCU::write(MEM_EP, src.as_ptr(), src.len(), off as GlobOff).unwrap();
    }

    fn read_reg(&self, reg: KeccakReg) -> Reg {
        let addr = self.addr + (reg as usize) * size_of::<Reg>();
        let val: Reg = TCU::read_obj(MMIO_EP, addr as GlobOff).unwrap();
        log!(LogFlags::SHA3Reg, "SHA3::read_reg({:?}) -> {:#x}", reg, val);
        val
    }

    fn write_reg(&self, reg: KeccakReg, value: Reg) {
        log!(
            LogFlags::SHA3Reg,
            "SHA3::write_reg({:?}) <- {:#x}",
            reg,
            value
        );
        let addr = self.addr + (reg as usize) * size_of::<Reg>();
        TCU::write_obj(MMIO_EP, &value, addr as GlobOff).unwrap();
    }
}
