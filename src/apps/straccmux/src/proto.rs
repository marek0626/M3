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

use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::DefaultReply;
use base::log;
use base::mem::{GlobOff, MsgBuf, VirtAddr};
use base::serialize::{Deserialize, Serialize};
use base::tcu;
use base::tcu::EpId;
use base::{build_vmsg, env};

use crate::aes::AESAcc;

const EP_IN_SEND: EpId = 16;
const EP_IN_MEM: EpId = 17;
const EP_OUT_SEND: EpId = 18;
const EP_OUT_MEM: EpId = 19;
const EP_RECV: EpId = 20;

const BUF_SIZE: usize = 4096;
const KEY_SIZE: usize = 16;
const FILE_RBUF_ADDR: VirtAddr = VirtAddr::new(0x14C00);

#[allow(unused)]
#[derive(Copy, Clone, Debug, Serialize)]
#[serde(crate = "base::serde")]
#[repr(u64)]
enum Ops {
    FStat,
    Seek,
    NextIn,
    NextOut,
    Commit,
}

#[derive(Clone, Debug, Serialize)]
#[serde(crate = "base::serde")]
struct NextInOutReq {
    opcode: Ops,
    fileid: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "base::serde")]
struct NextInOutReply {
    _res: Code,
    offset: usize,
    len: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(crate = "base::serde")]
struct CommitReq {
    opcode: Ops,
    fileid: u64,
    pos: usize,
}

#[derive(Default, Clone, Debug)]
struct FileView {
    off: usize,
    end: usize,
}

pub struct Executor {
    ep_off: tcu::EpId,
    input: FileView,
    output: FileView,
    aes: AESAcc,
}

impl Executor {
    pub fn new(ep_off: tcu::EpId) -> Self {
        Self {
            ep_off,
            input: FileView::default(),
            output: FileView::default(),
            aes: AESAcc,
        }
    }

    pub fn step(&mut self) -> bool {
        // request input
        if self.input.off == self.input.end {
            let reply = self.next_in();

            if reply.len == 0 {
                self.commit();
                return false;
            }

            self.input.off = reply.offset;
            self.input.end = reply.offset + reply.len;
        }

        // read block
        let amount = self.read_block();
        self.input.off += amount;

        // compute on block
        let mut pos = 16;
        while pos < amount {
            self.aes.encrypt(pos, (BUF_SIZE - 16) / 2 + pos);
            pos += 16;
        }

        // write output
        let mut pos = 0;
        while pos < amount {
            // request output?
            if self.output.off == self.output.end {
                let reply = self.next_out();
                self.output.off = reply.offset;
                self.output.end = reply.offset + reply.len;
            }

            // write block
            let written = self.write_block(amount - pos, pos);
            self.output.off += written;
            pos += written;
        }

        true
    }

    fn read_block(&self) -> usize {
        let amount = (self.input.end - self.input.off).min(Self::accel_inout_size());
        log!(
            LogFlags::Debug,
            "reading {} @ {:#x} (end={:#x})",
            amount,
            self.input.off,
            self.input.end,
        );

        tcu::TCU::read(
            self.ep_off + EP_IN_MEM,
            Self::accel_input_addr() as *mut u8,
            amount,
            self.input.off as GlobOff,
        )
        .unwrap();
        amount
    }

    fn write_block(&self, size: usize, pos: usize) -> usize {
        let amount = (self.output.end - self.output.off).min(size);
        log!(
            LogFlags::Debug,
            "writing {} @ {:#x} (end={:#x})",
            amount,
            self.output.off,
            self.output.end
        );
        tcu::TCU::write(
            self.ep_off + EP_OUT_MEM,
            (Self::accel_output_addr() + pos) as *const u8,
            amount,
            self.output.off as GlobOff,
        )
        .unwrap();
        amount
    }

    fn next_in(&self) -> NextInOutReply {
        self.send(EP_IN_SEND, NextInOutReq {
            opcode: Ops::NextIn,
            fileid: 0,
        })
        .unwrap();

        let reply = self.receive().unwrap();
        log!(LogFlags::Debug, "received {:?}", reply);
        reply
    }

    fn next_out(&self) -> NextInOutReply {
        self.send(EP_OUT_SEND, NextInOutReq {
            opcode: Ops::NextOut,
            fileid: 0,
        })
        .unwrap();

        let reply = self.receive().unwrap();
        log!(LogFlags::Debug, "received {:?}", reply);
        reply
    }

    fn commit(&self) {
        self.send(EP_OUT_SEND, CommitReq {
            opcode: Ops::Commit,
            fileid: 0,
            pos: self.output.off,
        })
        .unwrap();
        self.receive::<DefaultReply>().unwrap();
    }

    #[allow(unused)]
    fn accel_key_addr() -> usize {
        Self::accel_buf_addr()
    }

    fn accel_input_addr() -> usize {
        Self::accel_buf_addr() + KEY_SIZE
    }

    fn accel_output_addr() -> usize {
        Self::accel_buf_addr() + KEY_SIZE + Self::accel_inout_size()
    }

    fn accel_inout_size() -> usize {
        (BUF_SIZE - KEY_SIZE) / 2
    }

    fn accel_buf_addr() -> usize {
        env::boot().tile_desc().mem_size() - BUF_SIZE
    }

    fn send<M: Serialize>(&self, ep: tcu::EpId, msg: M) -> Result<(), Error> {
        let mut msg_buf = MsgBuf::borrow_def();
        build_vmsg!(&mut msg_buf, msg);
        tcu::TCU::send(self.ep_off + ep, &msg_buf, 0, self.ep_off + EP_RECV)
    }

    fn receive<'de, R: Deserialize<'de>>(&self) -> Result<R, Error> {
        super::receive(self.ep_off + EP_RECV, FILE_RBUF_ADDR)
    }
}
