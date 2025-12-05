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

use m3::client::EvidenceSession;
use m3::env;
use m3::errors::Error;
use m3::println;

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
