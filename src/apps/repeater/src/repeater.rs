/*
 * Copyright (C) 2021, Tendsin Mende <tendsin.mende@mailbox.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
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

use m3::col::{String, ToString, Vec};
use m3::com::{MemCap, MemGate};
use m3::errors::{Code, Error, VerboseError};
use m3::kif::Perm;
use m3::mem::GlobOff;
use m3::tiles::{ActivityArgs, ChildActivity, OwnActivity, RunningActivity, Tile, TileArgs};
use m3::time::{CycleInstant, Profiler};
use m3::util::parse;
use m3::vfs::{OpenFlags, VFS};
use m3::{client, env, vec};
use m3::{println, verror, wv_perf};

struct RepeaterSettings {
    repeats: u64,
    warmup: u64,
    args: Vec<String>,
    shmem: Vec<String>,
    mux_mem_size: Option<GlobOff>,
    tee: bool,
}

fn usage() -> ! {
    println!(
        "Usage: {} [options] <program> [<arg1>..]",
        env::args().next().unwrap()
    );
    println!();
    println!("    -r <repeats>  : set the number of repetitions (default: 1)");
    println!("    -w <warmups>  : set the number of warmup runs (default: 0)");
    println!("    -m <memsize>  : load app as multiplexer with <memsize> memory");
    println!("    -s <shmem>    : add app to shared memory with name <shmem>");
    println!("    -t            : run app as a TEE");
    OwnActivity::exit_with(Code::InvArgs);
}

fn parse_arg<T: core::str::FromStr>(arg: &str, name: &str) -> Result<T, VerboseError> {
    arg.parse::<T>()
        .map_err(|_| verror!(Code::InvArgs, "Could not parse {} '{}'", name, arg))
}

fn parse_args() -> Result<RepeaterSettings, VerboseError> {
    let args: Vec<&str> = env::args().collect();

    let mut settings = RepeaterSettings {
        repeats: 1,
        warmup: 0,
        args: vec![],
        shmem: vec![],
        mux_mem_size: None,
        tee: false,
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
            "-m" => {
                settings.mux_mem_size = Some(parse::size(&args[i + 1]).unwrap() as GlobOff);
            },
            "-s" => {
                settings.shmem.push(args[i + 1].to_string());
            },
            "-t" => {
                settings.tee = true;
                i -= 1; // no argument
            },
            _ => break,
        }
        i += 2;
    }
    if i >= args.len() {
        return Err(verror!(Code::InvArgs, "Missing arguments"));
    }
    settings.args = args.iter().skip(i).map(|s| s.to_string()).collect();

    Ok(settings)
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let settings = parse_args().unwrap_or_else(|e| {
        println!("Invalid arguments: {}", e);
        usage();
    });

    VFS::mount("/", client::M3FS_MAGIC, "m3fs").expect("mount root filesystem");

    let prof = Profiler::default()
        .warmup(settings.warmup)
        .repeats(settings.repeats);

    let res = prof.run::<CycleInstant, _>(|| {
        let tile = Tile::get_with(
            "compat",
            TileArgs::default()
                .init(settings.mux_mem_size.is_none())
                .inherit_pmp(settings.mux_mem_size.is_none()),
        )
        .expect("alloc tile");

        let _mux_mem = if let Some(mux_mem_size) = settings.mux_mem_size {
            let mux_mem = MemGate::new(mux_mem_size, Perm::RW).expect("alloc mux memory");

            let mut elf = VFS::open(&settings.args[0], OpenFlags::R).expect("open mux ELF");
            tile.load_mux(&settings.args[0], &mut elf, &mux_mem)
                .expect("load mux ELF");

            tile.start(Some(&mux_mem), 256).expect("start tile");

            Some(mux_mem)
        }
        else {
            None
        };

        let _shmems = if settings.tee {
            let shmems = settings
                .shmem
                .iter()
                .map(|name| {
                    let mcap = MemCap::new_shmem(name).expect("get shmem");
                    let mtile = Tile::new_from_shmem(name).expect("get memory tile");
                    mcap.make_exclusive(&mtile, &tile, true)
                        .expect("make exclusive");
                    (mcap, mtile)
                })
                .collect::<Vec<_>>();

            tile.lock().expect("tile lock");
            Some(shmems)
        }
        else {
            None
        };

        let mut act = ChildActivity::new_with(tile.clone(), ActivityArgs::new("child"))
            .expect("create activity");
        act.add_mount("/", "/");

        let act = if settings.mux_mem_size.is_some() {
            act.exec_file(None, &settings.args, || Ok(()))
                .expect("exec activity")
        }
        else {
            act.exec(&settings.args).expect("exec activity")
        };

        act.wait().expect("wait for activity");

        if settings.mux_mem_size.is_some() {
            drop(act);
            tile.stop().expect("tile reset");
        }
    });

    wv_perf!("run", res);

    Ok(())
}
