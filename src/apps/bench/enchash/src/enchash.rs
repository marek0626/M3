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

use m3core::cap::Selector;
use m3core::client::{HashInput, RoTSession};
use m3core::col::Vec;
use m3core::com::MemCap;
use m3core::crypto::HashAlgorithm;
use m3core::errors::{Code, Error};
use m3core::io::{Read, Write};
use m3core::kif::Perm;
use m3core::mem::GlobOff;
use m3core::println;
use m3core::rc::Rc;
use m3core::tiles::{ChildActivity, OwnActivity, RunningActivity, Tile, TileArgs};
use m3core::vfs::{File, FileRef, OpenFlags, VFS};
use m3core::{env, vec};

use accel::StreamAccel;
use pipecli::{IndirectPipe, Pipes};

create_heap!(64 * 1024);

const VERBOSE: bool = false;
const PIPE_SIZE: usize = 64 * 1024;
const BUF_SIZE: usize = 16 * 1024;

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    m3files::vfs_init().unwrap();
    m3core::env::init();
    m3core::env::run();
}

fn create_shm_pipe(
    pipes: &Pipes,
    name: &str,
    sel: Selector,
    tee: bool,
) -> (
    Option<Rc<Tile>>,
    IndirectPipe,
    FileRef<dyn File>,
    FileRef<dyn File>,
) {
    let (in_mem, in_tile) = if tee {
        let in_pipe =
            MemCap::new_shmem(name).unwrap_or_else(|e| panic!("get shmem '{}': {}", name, e));
        let in_tile =
            Tile::new_bind(sel).unwrap_or_else(|e| panic!("bind tile for '{}': {}", name, e));
        (in_pipe, Some(in_tile))
    }
    else {
        (
            MemCap::new(PIPE_SIZE as GlobOff, Perm::RW)
                .unwrap_or_else(|e| panic!("get shmem '{}': {}", name, e)),
            None,
        )
    };

    let in_pipe = IndirectPipe::new(&pipes, in_mem)
        .unwrap_or_else(|e| panic!("create pipe '{}': {}", name, e));
    let in_reader = in_pipe.reader().unwrap();
    let in_writer = in_pipe.writer().unwrap();
    (in_tile, in_pipe, in_reader, in_writer)
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 4 && args.len() != 5 {
        println!(
            "Usage: {} <data-size> <output> <pipe-sel-start> [-t]",
            args[0]
        );
        return Err(Error::new(Code::InvArgs));
    }

    let datasize = args[1].parse::<usize>().expect("Invalid data size");
    let outfile = args[2];
    let pipe_sels = args[3].parse::<Selector>().expect("Invalid pipe-sel-start");
    let tee = args.len() > 3 && args[3] == "-t";

    let pipes = Pipes::new("pipes").expect("open pipes session");

    let (in_tile, in_pipe, mut in_reader, mut in_writer) =
        create_shm_pipe(&pipes, "inpipe", pipe_sels + 0, tee);
    let (out_tile, out_pipe, mut out_reader, mut out_writer) =
        create_shm_pipe(&pipes, "outpipe", pipe_sels + 1, tee);
    let (_hash_tile, hash_pipe, mut hash_reader, mut hash_writer) =
        create_shm_pipe(&pipes, "hashpipe", pipe_sels + 2, tee);

    let tile = Tile::get_with(
        "riscv32+coreacc|core",
        TileArgs::default().inherit_pmp(false),
    )
    .expect("allocate riscv32 tile");
    let act = ChildActivity::new(tile.clone(), "test").expect("create child activity");

    let mut accel = StreamAccel::new(&act, tee)?;
    accel.attach_input(&mut in_reader).expect("attach input");
    accel.attach_output(&mut out_writer).expect("attach output");

    if tee {
        tile.lock().unwrap();

        let in_tile = in_tile.as_ref().unwrap();
        in_pipe
            .memory()
            .make_exclusive(in_tile, &tile, true)
            .expect("make in-pipe exclusive");
        let out_tile = out_tile.as_ref().unwrap();
        out_pipe
            .memory()
            .make_exclusive(out_tile, &tile, true)
            .expect("make out-pipe exclusive");
    }

    let run = act.start().expect("start activity");

    let mut output = VFS::open(
        outfile,
        OpenFlags::W | OpenFlags::CREATE | OpenFlags::NEW_SESS,
    )
    .unwrap_or_else(|_| panic!("creating {} for writing", outfile));

    let sha3 = RoTSession::new("hash", &HashAlgorithm::SHA3_224).unwrap();

    let in_data = vec![0u8; BUF_SIZE];
    let mut out_data = vec![0u8; BUF_SIZE];

    in_writer
        .set_blocking(false)
        .expect("make input non-blocking");
    out_reader
        .set_blocking(false)
        .expect("make output non-blocking");

    let mut read_pos = 0;
    let mut write_pos = 0;
    while read_pos < datasize {
        let mut progress = 0;

        if write_pos < datasize {
            let amount = (datasize - write_pos).min(in_data.len());
            match in_writer.write_all(&in_data[0..amount]) {
                Ok(_) => {
                    if VERBOSE {
                        println!("Wrote {} bytes", amount);
                    }
                    write_pos += amount;
                    if write_pos >= datasize {
                        in_pipe.close_writer();
                    }
                    progress += 1;
                },
                Err(e) if e.code() != Code::WouldBlock => panic!("write failed: {}", e),
                Err(_) => {},
            }
        }

        match out_reader.read(&mut out_data) {
            Ok(read) => {
                if VERBOSE {
                    println!("Read {} bytes", read);
                }

                hash_writer
                    .write_all(&out_data[..read])
                    .expect("write to-be-hashed chunk");
                hash_writer.flush().expect("flushing hash pipe");

                hash_reader
                    .hash_input_chunk(&sha3, read)
                    .expect("hash chunk");

                output
                    .write_all(&out_data[..read])
                    .expect("write to output file");

                progress += 1;
                read_pos += read;
            },
            Err(e) if e.code() != Code::WouldBlock => panic!("write failed: {}", e),
            Err(_) => {},
        }

        if read_pos < datasize && progress == 0 {
            OwnActivity::sleep().unwrap();
        }
    }

    hash_pipe.close_writer();
    out_pipe.close_reader();

    run.wait().expect("wait activity");

    let mut hash = [0u8; 28];
    sha3.finish(&mut hash).expect("get hash");
    println!("hash: {:?}", hash);

    // drop those explicitly before we drop `run` (the activity)
    drop(hash_pipe);
    drop(out_pipe);
    drop(in_pipe);
    drop(output);
    drop(accel);

    Ok(())
}
