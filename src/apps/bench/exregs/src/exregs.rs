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

use m3::col::Vec;
use m3::com::{GateCap, MemCap};
use m3::errors::{Code, Error, VerboseError};
use m3::kif::Perm;
use m3::mem::{GlobOff, VirtAddr};
use m3::tiles::{Activity, ChildActivity, OwnActivity, Tile, TileArgs};
use m3::time::{CycleInstant, Profiler};
use m3::{env, vec, wv_perf};
use m3::{println, verror};

#[derive(Debug)]
struct BenchSettings {
    repeats: u64,
    warmup: u64,
    exregs: u64,
    data_size: usize,
}

fn parse_arg<T: core::str::FromStr>(arg: &str, name: &str) -> Result<T, VerboseError> {
    arg.parse::<T>()
        .map_err(|_| verror!(Code::InvArgs, "Could not parse {} '{}'", name, arg))
}

fn parse_args() -> Result<BenchSettings, VerboseError> {
    let args: Vec<&str> = env::args().collect();

    let mut settings = BenchSettings {
        repeats: 1,
        warmup: 0,
        exregs: 0,
        data_size: 4096,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "-r" => {
                settings.repeats = parse_arg(args[i + 1], "repeats")?;
            },
            "-w" => {
                settings.warmup = parse_arg(args[i + 1], "warmups")?;
            },
            "-e" => {
                settings.exregs = parse_arg(args[i + 1], "exregs")?;
            },
            "-d" => {
                settings.data_size = parse_arg(args[i + 1], "exregs")?;
            },
            _ => break,
        }
        i += 2;
    }

    Ok(settings)
}

fn usage() -> ! {
    println!(
        "Usage: {} [-r <repeats>] [-w <warmups>] [-e <exregs>] [-d <data-size>]",
        env::args().next().unwrap()
    );
    OwnActivity::exit_with(Code::InvArgs);
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let settings = parse_args().unwrap_or_else(|e| {
        println!("Invalid arguments: {}", e);
        usage();
    });

    let tile = Tile::get_with(
        "riscv32+coreacc|core",
        TileArgs::default().inherit_pmp(false),
    )
    .expect("allocate riscv32 tile");

    let act = ChildActivity::new(tile.clone(), "test").expect("create child activity");

    let mem = MemCap::new_foreign(
        act.sel(),
        VirtAddr::from(0),
        tile.desc().mem_size() as GlobOff,
        Perm::RW,
    )
    .expect("get SPM cap");

    for i in 0..settings.exregs {
        let locked = i == settings.exregs - 1;
        mem.make_exclusive(&tile, Activity::own().tile(), locked)
            .expect("make SPM exclusive");
    }

    let mgate = mem.activate().expect("activate SPM cap");
    let mut buf = vec![0u8; settings.data_size];

    let prof = Profiler::default()
        .warmup(settings.warmup)
        .repeats(settings.repeats);
    let res = prof.run::<CycleInstant, _>(|| {
        mgate.read(&mut buf, 0).expect("read");
    });
    wv_perf!("SPM access", res);

    Ok(())
}
