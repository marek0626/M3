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

use m3::cap::Selector;
use m3::com::{recv_msg, RecvCap, RecvGate, SGateArgs, SendGate};
use m3::env;
use m3::errors::{Code, Error};
use m3::test::{DefaultWvTester, WvTester};
use m3::tiles::{Activity, ActivityArgs, ChildActivity, RunningActivity, Tile};
use m3::util::math;

use m3::{send_vmsg, wv_assert_eq, wv_assert_ok, wv_require_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, run_arguments);
    wv_run_test!(t, run_nested);
    wv_run_test!(t, run_send_receive);
    wv_run_test!(t, exec_fail);
    wv_run_test!(t, exec_hello);
    wv_run_test!(t, exec_rust_hello);
}

fn run_arguments(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));

    let act = wv_require_ok!(act.run(|| {
        let mut t = DefaultWvTester::default();
        wv_assert_eq!(t, env::args().count(), 1);
        assert!(env::args().next().is_some());
        assert!(env::args().next().unwrap().ends_with("rustmisctests"));
        Ok(())
    }));

    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}

fn run_nested(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let mut act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));
    act.add_mount("/", "/");

    let act = wv_require_ok!(act.run(|| {
        let mut t = DefaultWvTester::default();
        let tile = wv_require_ok!(Tile::get("compat|own"));
        let act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));

        let act = wv_require_ok!(act.run(|| { Ok(()) }));

        wv_assert_eq!(t, act.wait(), Ok(Code::Success));
        Ok(())
    }));

    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}

fn run_send_receive(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let mut act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));

    let rgate = wv_require_ok!(RecvCap::new(math::next_log2(256), math::next_log2(256)));

    wv_assert_ok!(t, act.delegate_obj(rgate.sel()));

    let mut dst = act.data_sink();
    dst.push(rgate.sel());

    let act = wv_require_ok!(act.run(|| {
        let mut t = DefaultWvTester::default();
        let mut src = Activity::own().data_source();
        let rg_sel: Selector = src.pop().unwrap();

        let rgate = wv_require_ok!(RecvGate::new_bind(rg_sel));
        let mut res = wv_require_ok!(recv_msg(&rgate));
        let i1 = wv_require_ok!(res.pop::<u32>());
        let i2 = wv_require_ok!(res.pop::<u32>());
        wv_assert_eq!(t, (i1, i2), (42, 23));
        Err(Error::new(Code::NoFreeTile))
    }));

    let sgate = wv_require_ok!(SendGate::new_with(SGateArgs::new(&rgate).credits(1)));
    wv_assert_ok!(t, send_vmsg!(&sgate, RecvGate::def(), 42, 23));

    wv_assert_eq!(t, act.wait(), Ok(Code::NoFreeTile));
}

fn exec_fail(_t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    // file too small
    {
        let act = wv_require_ok!(ChildActivity::new_with(
            tile.clone(),
            ActivityArgs::new("test")
        ));
        let act = act.exec(&["/testfile.txt"]);
        assert!(act.is_err() && act.err().unwrap().code() == Code::EndOfFile);
    }

    // not an ELF file
    {
        let act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));
        let act = act.exec(&["/pat.bin"]);
        assert!(act.is_err() && act.err().unwrap().code() == Code::InvalidElf);
    }
}

fn exec_hello(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));

    let act = wv_require_ok!(act.exec(&["/bin/hello"]));
    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}

fn exec_rust_hello(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let act = wv_require_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));

    let act = wv_require_ok!(act.exec(&["/bin/rusthello"]));
    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}
