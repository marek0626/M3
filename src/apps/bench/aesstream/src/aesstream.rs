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
extern crate unimux;

use heapsimple::create_heap;

use m3core::col::Vec;
use m3core::env;
use m3core::errors::{Code, Error};
use m3core::println;
use m3core::tiles::{ChildActivity, RunningActivity, Tile, TileArgs};
use m3core::vfs::{OpenFlags, VFS};

use accel::StreamAccel;

create_heap!(64 * 1024);

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    m3files::vfs_init().unwrap();
    m3core::env::init();
    m3core::env::run();
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        println!("Usage: {} <input> <output>", args[0]);
        return Err(Error::new(Code::InvArgs));
    }

    let infile = args[1];
    let outfile = args[2];

    let tile = Tile::get_with(
        "riscv32+coreacc|core",
        TileArgs::default().inherit_pmp(false),
    )
    .expect("allocate riscv32 tile");
    let act = ChildActivity::new(tile, "test").expect("create child activity");

    let mut accel = StreamAccel::new(&act)?;
    let mut input = VFS::open(infile, OpenFlags::R | OpenFlags::NEW_SESS)
        .unwrap_or_else(|_| panic!("open {} for reading", infile));
    let mut output = VFS::open(
        outfile,
        OpenFlags::W | OpenFlags::CREATE | OpenFlags::NEW_SESS,
    )
    .unwrap_or_else(|_| panic!("creating {} for writing", outfile));
    accel.attach_input(&mut input).expect("attach input");
    accel.attach_output(&mut output).expect("attach output");

    let run = act.start().expect("start activity");
    run.wait().expect("wait activity");

    // drop those explicitly before we drop `run` (the activity)
    drop(output);
    drop(input);
    drop(accel);

    Ok(())
}
