/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
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

use core::str::FromStr;

use m3::com::MemGate;
use m3::env;
use m3::errors::{Code, Error};
use m3::kif;
use m3::mem::GlobOff;
use m3::time::{CycleInstant, Profiler};
use m3::util::random::LinearCongruentialGenerator;
use m3::{format, vec, wv_perf};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum Mode {
    Copy,
    TCU,
    Random,
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcu" => Ok(Self::TCU),
            "copy" => Ok(Self::Copy),
            "rand" => Ok(Self::Random),
            _ => Err(Error::new(Code::InvArgs)),
        }
    }
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mode: Mode = env::args().nth(1).unwrap().parse().unwrap();
    let size = env::args().nth(2).map(|s| s.parse::<usize>().unwrap());

    let buf = vec![0u8; 1024 * 1024];
    let mut buf2 = vec![0u8; 1024 * 1024];

    let mgate = MemGate::new(buf.len() as GlobOff, kif::Perm::W).expect("Unable to create mgate");

    if let Some(size) = size {
        perform_op(&mgate, &buf, &mut buf2, size, mode);
    }
    else {
        for i in 0..=28 {
            perform_op(&mgate, &buf, &mut buf2, 1 << i, mode);
        }
    }
    Ok(())
}

fn perform_op(mgate: &MemGate, buf: &[u8], buf2: &mut [u8], size: usize, mode: Mode) {
    let prof = Profiler::default().repeats(10).warmup(2);
    let cur_buf = &buf[0..buf.len().min(size)];

    wv_perf!(
        format!("{:?}-op of {}b with {}b buf", mode, size, cur_buf.len()),
        prof.run::<CycleInstant, _>(|| {
            let mut rng = LinearCongruentialGenerator::new(0xDEAD);
            let mut total = 0;
            while total < size {
                match mode {
                    Mode::TCU => mgate.write(cur_buf, 0).expect("Writing failed"),
                    Mode::Copy => buf2[0..cur_buf.len()].copy_from_slice(cur_buf),
                    Mode::Random => {
                        const CHUNK_SIZE: usize = 64;
                        let chunks = buf.len() / CHUNK_SIZE;
                        for _ in 0..chunks {
                            let a = rng.get() as usize % chunks;
                            let b = rng.get() as usize % chunks;
                            buf2[(a * CHUNK_SIZE)..((a + 1) * CHUNK_SIZE)].copy_from_slice(
                                &cur_buf[(b * CHUNK_SIZE)..((b + 1) * CHUNK_SIZE)],
                            );
                        }
                    },
                }
                total += buf.len();
            }
        })
    );
}
