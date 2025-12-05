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
use m3::col::Vec;
use m3::com::MemCap;
use m3::errors::{Code, Error};
use m3::io::{Read, Write};
use m3::kif::Perm;
use m3::mem::GlobOff;
use m3::println;
use m3::rc::Rc;
use m3::tiles::{ChildActivity, OwnActivity, RunningActivity, RunningDeviceActivity, Tile};
use m3::time::CycleDuration;
use m3::vfs::{File, FileRef};
use m3::{env, vec};

use accel::StreamAccel;
use pipecli::{IndirectPipe, Pipes};

const VERBOSE: bool = false;
const PIPE_SIZE: usize = 64 * 1024;
const BUF_SIZE: usize = 16 * 1024;
const ACCEL_COUNT: usize = 3;

// the time for one 2048 block for 2D-FFT; determined by ALADDIN and
// picking the sweet spot between area, power and performance.
// 732 cycles for the FFT function. we have two loops in FFT2D with
// 16 iterations each. we unroll both 4 times, leading to
// (4 + 4) * 732 = 5856.

const ACCEL_TIMES: [CycleDuration; ACCEL_COUNT] = [
    CycleDuration::new(5856 / 2), // FFT
    CycleDuration::new(1189 / 2), // MUL
    CycleDuration::new(5856 / 2), // IFFT
];

struct AccelActivity {
    accel: StreamAccel,
    act: ChildActivity,
    tile: Rc<Tile>,
}

impl AccelActivity {
    fn new(name: &str, _comp_time: CycleDuration, tee: bool) -> Result<Self, Error> {
        let tile = Tile::get("copy")?;
        let act = ChildActivity::new(tile.clone(), name)?;
        let accel = StreamAccel::new(&act, tee)?;
        Ok(Self { tile, act, accel })
    }
}

struct Chain {
    accels: Vec<AccelActivity>,
}

impl Chain {
    fn new(
        input: &mut FileRef<dyn File>,
        output: &mut FileRef<dyn File>,
        tee: bool,
    ) -> Result<Self, Error> {
        let mut fft = AccelActivity::new("FFT", ACCEL_TIMES[0], tee)?;
        let mut mul = AccelActivity::new("MUL", ACCEL_TIMES[1], tee)?;
        let mut ifft = AccelActivity::new("IFFT", ACCEL_TIMES[2], tee)?;

        fft.accel.attach_input(input)?;
        fft.accel.attach_output_accel(&mul.accel)?;

        mul.accel.attach_input_accel(&fft.accel)?;
        mul.accel.attach_output_accel(&ifft.accel)?;

        ifft.accel.attach_input_accel(&mul.accel)?;
        ifft.accel.attach_output(output)?;

        Ok(Self {
            accels: vec![fft, mul, ifft],
        })
    }

    fn lock(
        &self,
        inmem: &MemCap,
        intile: &Rc<Tile>,
        outmem: &MemCap,
        outtile: &Rc<Tile>,
    ) -> Result<(), Error> {
        // our tile has already access, because repeater did it for us
        inmem.make_exclusive(intile, &self.accels[0].tile, true)?;
        outmem.make_exclusive(outtile, &self.accels[ACCEL_COUNT - 1].tile, true)?;
        for accel in &self.accels {
            accel.tile.lock()?;
        }
        Ok(())
    }

    fn start(self) -> Result<RunningChain, Error> {
        let accels = self
            .accels
            .into_iter()
            .map(|a| {
                let act = a.act.start().unwrap();
                RunningAccelActivity {
                    tile: a.tile,
                    act,
                    accel: a.accel,
                }
            })
            .collect();
        Ok(RunningChain { accels })
    }
}

#[allow(unused)]
struct RunningAccelActivity {
    accel: StreamAccel,
    act: RunningDeviceActivity,
    tile: Rc<Tile>,
}

struct RunningChain {
    accels: Vec<RunningAccelActivity>,
}

impl RunningChain {
    fn wait(&self) -> Result<(), Error> {
        for accel in &self.accels {
            accel.act.wait()?;
        }
        Ok(())
    }
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 4 && args.len() != 5 {
        println!(
            "Usage: {} <image-size> <inpipe-sel> <outpipe-sel> [-t]",
            args[0]
        );
        return Err(Error::new(Code::InvArgs));
    }

    let imgsize = args[1].parse::<usize>().expect("Parse image size");
    let inpipe_sel = args[2].parse::<Selector>().expect("Parse inpipe selector");
    let outpipe_sel = args[3].parse::<Selector>().expect("Parse outpipe selector");
    let tee = args.len() > 4 && args[4] == "-t";

    let pipes = Pipes::new("pipes").expect("open pipes session");
    let (in_mem, in_tile) = if tee {
        let in_pipe = MemCap::new_shmem("inpipe").expect("get inpipe shmem");
        let in_tile = Tile::new_bind(inpipe_sel).expect("bind inpipe");
        (in_pipe, Some(in_tile))
    }
    else {
        (
            MemCap::new(PIPE_SIZE as GlobOff, Perm::RW).expect("get inpipe shmem"),
            None,
        )
    };
    let in_pipe = IndirectPipe::new(&pipes, in_mem).expect("create inpipe");
    let mut in_reader = in_pipe.reader().unwrap();
    let mut in_writer = in_pipe.writer().unwrap();

    let (out_mem, out_tile) = if tee {
        let out_pipe = MemCap::new_shmem("outpipe").expect("get outpipe shmem");
        let out_tile = Tile::new_bind(outpipe_sel).expect("bind outpipe");
        (out_pipe, Some(out_tile))
    }
    else {
        (
            MemCap::new(PIPE_SIZE as GlobOff, Perm::RW).expect("get outpipe shmem"),
            None,
        )
    };
    let out_pipe = IndirectPipe::new(&pipes, out_mem).expect("create outpipe");
    let mut out_reader = out_pipe.reader().unwrap();
    let mut out_writer = out_pipe.writer().unwrap();

    let chain = Chain::new(&mut in_reader, &mut out_writer, tee).expect("create chain");
    if tee {
        chain
            .lock(
                in_pipe.memory(),
                in_tile.as_ref().unwrap(),
                out_pipe.memory(),
                out_tile.as_ref().unwrap(),
            )
            .expect("lock chain");
    }
    let chain = chain.start().expect("start chain");

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
    while read_pos < imgsize {
        let mut progress = 0;

        if write_pos < imgsize {
            match in_writer.write_all(&in_data) {
                Ok(_) => {
                    if VERBOSE {
                        println!("Wrote {} bytes", in_data.len());
                    }
                    write_pos += in_data.len();
                    if write_pos >= imgsize {
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
                progress += 1;
                read_pos += read;
            },
            Err(e) if e.code() != Code::WouldBlock => panic!("write failed: {}", e),
            Err(_) => {},
        }

        if read_pos < imgsize && progress == 0 {
            OwnActivity::sleep().unwrap();
        }
    }

    out_pipe.close_reader();

    chain.wait().expect("wait");

    drop(in_pipe);
    drop(out_pipe);

    Ok(())
}
