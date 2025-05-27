/*
 * Copyright (C) 2025 Nils Asmussen, Barkhausen Institut
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
use m3core::client::EvidenceSession;
use m3core::env;
use m3core::errors::Error;
use m3core::println;

create_heap!(64 * 1024);

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    m3core::env::init();
    m3core::env::run();
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let att_id = env::args().nth(1).unwrap().parse::<u32>().unwrap();

    let ev = EvidenceSession::new("evidence")?;

    println!("req quote");

    let nonce = 0xDEAD_BEEF;
    let quote = ev.quote(att_id, nonce)?;

    println!("received quote: {}", quote);
    Ok(())
}
