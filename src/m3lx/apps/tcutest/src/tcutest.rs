/*
 * Copyright (C) 2023 Nils Asmussen, Barkhausen Institut
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

extern crate m3impl as m3;

use m3::com::{recv_msg, MemGate, RecvGate, SendGate};
use m3::errors::Code;
use m3::kif::Perm;
use m3::test::{DefaultWvTester, WvTester};
use m3::{reply_vmsg, send_recv, wv_assert_eq, wv_assert_ok};

fn main() -> Result<(), std::io::Error> {
    m3::env::init();

    let mut tester = DefaultWvTester::default();

    let mut buf = vec![0u32; 128];
    let mgate = wv_assert_ok!(MemGate::new(0x4000, Perm::RW));
    wv_assert_ok!(mgate.write(&buf, 0));

    let mut last = 0;
    for _ in 0..10 {
        wv_assert_ok!(mgate.read(&mut buf, 0));
        for j in 0..128 {
            wv_assert_eq!(tester, buf[j], last);
            buf[j] += 1;
        }
        wv_assert_ok!(mgate.write(&buf, 0));
        last += 1;
    }

    if m3::env::args().nth(1).unwrap() == "sender" {
        let sgate = wv_assert_ok!(SendGate::new_named("chan"));

        let mut val: u32 = 42;
        for _ in 0..16 {
            println!("Sending {}", val);
            wv_assert_ok!(send_recv!(&sgate, RecvGate::def(), val));
            val += 1;
        }
    }
    else {
        let rgate = wv_assert_ok!(RecvGate::new_named("chan"));

        for _ in 0..32 {
            let mut msg = wv_assert_ok!(recv_msg(&rgate));
            let val: u32 = wv_assert_ok!(msg.pop());
            println!("Received {} from {}", val, msg.label());
            wv_assert_ok!(reply_vmsg!(msg, Code::Success));
        }
    }

    Ok(())
}
