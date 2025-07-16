/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
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
use m3core::com::{MemGate, RecvBuf, RecvCap, RecvGate, SendGate};
use m3core::errors::Error;
use m3core::kif::Perm;
use m3core::test::{DefaultWvTester, WvTester};
use m3core::time::{CycleInstant, Profiler, Runner};
use m3core::{println, wv_perf, wv_require_ok, wv_run_suite, wv_run_test};

create_heap!(64 * 1024);

#[no_mangle]
pub extern "C" fn env_run() -> ! {
    m3core::env::init();
    m3core::env::run();
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, chan_create);
    println!("{}", tester);
    Ok(())
}

fn chan_create(t: &mut dyn WvTester) {
    wv_run_test!(t, mem);
    wv_run_test!(t, rcv);
    wv_run_test!(t, snd);
}

fn mem(_t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(100);

    struct Tester {
        base_mgate: MemGate,
        mgate: Option<MemGate>,
    }

    impl Runner for Tester {
        fn run(&mut self) {
            self.mgate = Some(wv_require_ok!(self.base_mgate.derive(0, 0x1000, Perm::RW)));
        }

        fn post(&mut self) {
            self.mgate.take();
        }
    }

    wv_perf!(
        "snd",
        prof.runner::<CycleInstant, _>(&mut Tester {
            base_mgate: wv_require_ok!(MemGate::new(0x1000, Perm::RW)),
            mgate: None,
        })
    );
}

fn rcv(_t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(100);

    struct Tester {
        rbuf: RecvBuf,
        rgate: Option<RecvGate>,
    }

    impl Runner for Tester {
        fn run(&mut self) {
            let rcap = wv_require_ok!(RecvCap::new(6, 6));
            let rgate = wv_require_ok!(rcap.activate_with(
                self.rbuf.mem(),
                self.rbuf.off(),
                self.rbuf.addr(),
                None
            ));
            self.rgate = Some(rgate);
        }

        fn post(&mut self) {
            self.rgate.take();
        }
    }

    wv_perf!(
        "rcv",
        prof.runner::<CycleInstant, _>(&mut Tester {
            rbuf: wv_require_ok!(RecvBuf::new(64)),
            rgate: None
        })
    );
}

fn snd(_t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(100);

    struct Tester {
        rgate: Option<RecvGate>,
        sgate: Option<SendGate>,
    }

    impl Runner for Tester {
        fn pre(&mut self) {
            if self.rgate.is_none() {
                self.rgate = Some(wv_require_ok!(RecvGate::new(10, 10)));
            }
        }

        fn run(&mut self) {
            self.sgate = Some(wv_require_ok!(SendGate::new(self.rgate.as_ref().unwrap())));
        }

        fn post(&mut self) {
            self.sgate.take();
        }
    }

    wv_perf!(
        "mem",
        prof.runner::<CycleInstant, _>(&mut Tester {
            rgate: None,
            sgate: None,
        })
    );
}
