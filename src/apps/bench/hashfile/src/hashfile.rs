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

#[allow(unused_extern_crates)]
extern crate unimux;

use heapsimple::create_heap;

use m3core::client::{HashInput, RoTSession};
use m3core::crypto::HashAlgorithm;
use m3core::errors::Error;
use m3core::vfs::{OpenFlags, VFS};
use m3core::{env, println};

create_heap!(64 * 1024);

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    m3files::vfs_init().unwrap();
    m3core::env::init();
    m3core::env::run();
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let input_size = env::args()
        .nth(1)
        .unwrap()
        .parse::<usize>()
        .expect("Invalid input size");

    let mut input =
        VFS::open("/data/4096k.txt", OpenFlags::R | OpenFlags::NEW_SESS).expect("open input file");

    let sha3 = RoTSession::new("hash2", &HashAlgorithm::SHA3_224).unwrap();
    input.hash_input(&sha3, input_size).expect("hash file");

    let hash = sha3.get_hash().expect("get hash");
    println!("hash: {:?}", hash);

    Ok(())
}
