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

use m3::client::{HashInput, HashOutput, RoTSession};
use m3::com::{GateCap, MemCap};
use m3::crypto::HashAlgorithm;
use m3::errors::{Code, Error};
use m3::io::Write;
use m3::test::{DefaultWvTester, WvTester};
use m3::time::{CycleInstant, Profiler};
use m3::vfs::{File, FileEvent, FileWaiter};
use m3::{
    println, wv_assert_eq, wv_assert_ok, wv_perf, wv_require_ok, wv_require_some, wv_run_test,
};

use hex_literal::hex;

use pipecli::{IndirectPipe, Pipes};

const PIPE_SHAKE_SIZE: usize = 256 * 1024; // 256 KiB

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_test!(&mut tester, shake_and_hash_pipe);
    println!("{}", tester);
    Ok(())
}

fn shake_and_hash_pipe(t: &mut dyn WvTester) {
    let prof = Profiler::default().warmup(1).repeats(10);
    let res = prof.run::<CycleInstant, _>(|| run_pipe(t));
    wv_perf!("shake_and_hash", res);
}

// echo Pipe! | hashsum shake128 -O 262144 -o - | hashsum sha3-256
fn run_pipe(t: &mut dyn WvTester) {
    let pipes = wv_require_ok!(Pipes::new("pipes"));

    // Create two pipes
    let imcap = wv_require_ok!(MemCap::new_shmem("inpipe"));
    let imgate = wv_require_ok!(imcap.activate());
    let ipipe = wv_require_ok!(IndirectPipe::new(&pipes, imgate));
    let omcap = wv_require_ok!(MemCap::new_shmem("outpipe"));
    let omgate = wv_require_ok!(omcap.activate());
    let opipe = wv_require_ok!(IndirectPipe::new(&pipes, omgate));

    let shake = wv_require_ok!(RoTSession::new("hash1", &HashAlgorithm::SHAKE128));
    let sha3 = wv_require_ok!(RoTSession::new("hash2", &HashAlgorithm::SHA3_256));

    {
        // echo "Pipe!"
        let mut ifile = wv_require_some!(ipipe.writer());
        wv_assert_ok!(t, writeln!(ifile, "Pipe!"));
        ipipe.close_writer();
    }

    let mut ipipe_reader = ipipe.reader().unwrap();
    let mut opipe_writer = opipe.writer().unwrap();
    let mut opipe_reader = opipe.reader().unwrap();

    wv_assert_ok!(t, ipipe_reader.set_blocking(false));
    wv_assert_ok!(t, opipe_writer.set_blocking(false));
    wv_assert_ok!(t, opipe_reader.set_blocking(false));

    let mut waiter = FileWaiter::default();
    waiter.add(ipipe_reader.fd(), FileEvent::INPUT);
    waiter.add(opipe_writer.fd(), FileEvent::OUTPUT);
    waiter.add(opipe_reader.fd(), FileEvent::INPUT);

    let mut read_eof = false;
    let mut rem_shake_size = PIPE_SHAKE_SIZE;
    loop {
        // "hashsum shake128 -O 262144 -o -"
        if !read_eof {
            match ipipe_reader.hash_input_chunk(&shake, usize::MAX) {
                Ok(0) => {
                    read_eof = true;
                    ipipe.close_reader();
                },
                Ok(_) => {},
                Err(e) if e.code() == Code::WouldBlock => {},
                Err(e) => panic!("Got error {}", e),
            }
        }
        if rem_shake_size > 0 {
            match opipe_writer.hash_output_chunk(&shake, rem_shake_size) {
                Ok(res) => {
                    rem_shake_size -= res;
                    if rem_shake_size == 0 {
                        opipe.close_writer();
                    }
                },
                Err(e) if e.code() == Code::WouldBlock => {},
                Err(e) => panic!("Got error {}", e),
            }
        }

        // hashsum sha3-256
        match opipe_reader.hash_input_chunk(&sha3, usize::MAX) {
            Ok(0) => break,
            Ok(_) => {},
            Err(e) if e.code() == Code::WouldBlock => {},
            Err(e) => panic!("Got error {}", e),
        }

        waiter.wait();
    }

    let mut buf = [0u8; HashAlgorithm::SHA3_256.output_bytes];
    wv_assert_ok!(t, sha3.finish(&mut buf));
    wv_assert_eq!(
        t,
        &buf,
        &hex!("dd20e9da838d0643a6d0e8af3ebbcac44692a32d595acd626e993dca02620aee")
    );
}
