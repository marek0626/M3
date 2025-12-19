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

use m3::cap::Selector;
use m3::client::{HashInput, RoTSession};
use m3::col::Vec;
use m3::com::MemCap;
use m3::crypto::HashAlgorithm;
use m3::errors::{Code, Error};
use m3::io::{LogFlags, Read, Write};
use m3::kif::Perm;
use m3::mem::GlobOff;
use m3::rc::Rc;
use m3::tiles::{ChildActivity, OwnActivity, RunningActivity, Tile, TileArgs};
use m3::vfs::{File, FileRef, OpenFlags, VFS};
use m3::{env, vec};
use m3::{log, println};

use accel::StreamAccel;
use pipecli::{IndirectPipe, Pipes};

const VERBOSE: bool = false;
const PIPE_SIZE: usize = 64 * 1024;
const BUF_SIZE: usize = 16 * 1024;

struct ShmPipe {
    tile: Option<Rc<Tile>>,
    pipe: IndirectPipe,
    reader: FileRef<dyn File>,
    writer: FileRef<dyn File>,
}

impl ShmPipe {
    fn new(pipes: &Pipes, name: &str, sel: Selector, tee: bool) -> Self {
        let (mem, tile) = if tee {
            let pipe =
                MemCap::new_shmem(name).unwrap_or_else(|e| panic!("get shmem '{}': {}", name, e));
            let tile =
                Tile::new_bind(sel).unwrap_or_else(|e| panic!("bind tile for '{}': {}", name, e));
            (pipe, Some(tile))
        }
        else {
            (
                MemCap::new(PIPE_SIZE as GlobOff, Perm::RW)
                    .unwrap_or_else(|e| panic!("get shmem '{}': {}", name, e)),
                None,
            )
        };

        let pipe = IndirectPipe::new(pipes, mem)
            .unwrap_or_else(|e| panic!("create pipe '{}': {}", name, e));
        let reader = pipe.reader().unwrap();
        let writer = pipe.writer().unwrap();
        Self {
            tile,
            pipe,
            reader,
            writer,
        }
    }
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
    let tee = args.len() > 4 && args[4] == "-t";

    let pipes = Pipes::new("pipes").expect("open pipes session");

    let mut input = ShmPipe::new(&pipes, "inpipe", pipe_sels + 0, tee);
    let mut output = ShmPipe::new(&pipes, "outpipe", pipe_sels + 1, tee);
    let mut hash = ShmPipe::new(&pipes, "hashpipe", pipe_sels + 2, tee);

    let tile = Tile::get_with(
        "riscv32+coreacc|core",
        TileArgs::default().inherit_pmp(false),
    )
    .expect("allocate riscv32 tile");
    let act = ChildActivity::new(tile.clone(), "test").expect("create child activity");

    let mut accel = StreamAccel::new(&act, tee)?;
    accel.attach_input(&mut input.reader).expect("attach input");
    accel
        .attach_output(&mut output.writer)
        .expect("attach output");

    if tee {
        tile.lock().unwrap();

        let in_tile = input.tile.as_ref().unwrap();
        input
            .pipe
            .memory()
            .make_exclusive(in_tile, &tile, true)
            .expect("make in-pipe exclusive");
        let out_tile = output.tile.as_ref().unwrap();
        output
            .pipe
            .memory()
            .make_exclusive(out_tile, &tile, true)
            .expect("make out-pipe exclusive");
    }

    let run = act.start().expect("start activity");

    let mut out_file = VFS::open(
        outfile,
        OpenFlags::W | OpenFlags::CREATE | OpenFlags::NEW_SESS,
    )
    .unwrap_or_else(|_| panic!("creating {} for writing", outfile));

    let sha3 = RoTSession::new("hash", &HashAlgorithm::SHA3_224).unwrap();

    let in_data = vec![0u8; BUF_SIZE];
    let mut out_data = vec![0u8; BUF_SIZE];

    input
        .writer
        .set_blocking(false)
        .expect("make input non-blocking");
    output
        .reader
        .set_blocking(false)
        .expect("make output non-blocking");

    let mut read_pos = 0;
    let mut write_pos = 0;
    while read_pos < datasize {
        let mut progress = 0;

        if write_pos < datasize {
            let amount = (datasize - write_pos).min(in_data.len());
            match input.writer.write_all(&in_data[0..amount]) {
                Ok(_) => {
                    if VERBOSE {
                        log!(LogFlags::Info, "Wrote {} bytes", amount);
                    }
                    write_pos += amount;
                    // TODO add occasional print to prevent halt
                    if write_pos > 0 && write_pos % 16384 == 0 {
                        println!("Wrote {} bytes", write_pos);
                    }
                    if write_pos >= datasize {
                        input.pipe.close_writer();
                    }
                    progress += 1;
                },
                Err(e) if e.code() != Code::WouldBlock => panic!("write failed: {}", e),
                Err(_) => {},
            }
        }

        match output.reader.read(&mut out_data) {
            Ok(read) => {
                if VERBOSE {
                    log!(LogFlags::Info, "Read {} bytes", read);
                }

                hash.writer
                    .write_all(&out_data[..read])
                    .expect("write to-be-hashed chunk");
                hash.writer.flush().expect("flushing hash pipe");

                hash.reader
                    .hash_input_chunk(&sha3, read)
                    .expect("hash chunk");

                out_file
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

    hash.pipe.close_writer();
    output.pipe.close_reader();

    run.wait().expect("wait activity");

    let mut res = [0u8; 28];
    sha3.finish(&mut res).expect("get hash");
    assert_eq!(res[0], 0);

    // drop those explicitly before we drop `run` (the activity)
    drop(hash);
    drop(output);
    drop(input);
    drop(out_file);
    drop(accel);

    Ok(())
}
