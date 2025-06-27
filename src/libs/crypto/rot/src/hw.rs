/*
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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

use core::cmp::min;

use base::crypto::HashType;
use base::mem::GlobOff;
use base::tcu::{ActId, EpId, TCU};
use kecacc::KecAcc;

pub const FLASH_EP: EpId = 0;
pub const TCU_ACT_ID: ActId = 0xffff;

pub static KECACC: KecAcc = KecAcc::new();

pub fn hash(ty: HashType, data: &[u8], out_hash: &mut [u8]) {
    KECACC.start_init(ty);
    KECACC.start_absorb_last(data);
    KECACC.start_pad();
    KECACC.start_squeeze(out_hash);
    KECACC.poll_complete_barrier();
}

pub fn copy_and_hash(
    ty: HashType,
    from_ep: EpId,
    to_ep: EpId,
    to_off: GlobOff,
    mut size: usize,
    temp_buf: &mut [u8],
    out_hash: &mut [u8],
) {
    KECACC.start_init(ty);

    let tmp_len = temp_buf.len();
    let mut off = 0;
    while size > 0 {
        let len = min(size, tmp_len);
        TCU::read(from_ep, temp_buf.as_mut_ptr(), len, off).expect("Failed to read via TCU");
        if len == size {
            KECACC.start_absorb_last(&temp_buf[..len]);
        }
        else {
            KECACC.start_absorb(&temp_buf[..len]);
        }
        KECACC.poll_complete();
        TCU::write(to_ep, temp_buf.as_ptr(), len, off + to_off).expect("Failed to write via TCU");
        off += len as GlobOff;
        size -= len;
    }
    KECACC.start_pad();
    KECACC.start_squeeze(out_hash);
    KECACC.poll_complete_barrier();
}
