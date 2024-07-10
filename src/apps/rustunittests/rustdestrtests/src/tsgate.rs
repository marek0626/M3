/*
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

use m3::cap::SelSpace;
use m3::com::{RGateArgs, RecvCap, Semaphore, SendGate};
use m3::errors::Code;
use m3::test::{DefaultWvTester, WvTester};
use m3::tiles::{Activity, ActivityArgs, ChildActivity, OwnActivity, RunningActivity, Tile};
use m3::time::TimeDuration;
use m3::{wv_assert_err, wv_assert_ok, wv_require_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, destroy);
}

fn destroy(t: &mut dyn WvTester) {
    let c2p_sem = wv_require_ok!(Semaphore::create(0));
    let child_sel = SelSpace::get().alloc_sel();

    let tile = wv_require_ok!(Tile::get("compat|own"));
    let mut child = wv_require_ok!(ChildActivity::new_with(
        tile,
        // ensure that child_sel is not reused by the child
        ActivityArgs::new("child").first_sel(child_sel + 1)
    ));
    wv_assert_ok!(t, child.delegate_obj(c2p_sem.sel()));

    let mut dst = child.data_sink();
    dst.push(c2p_sem.sel());
    dst.push(child_sel);

    let act = wv_require_ok!(child.run(|| {
        let mut t = DefaultWvTester::default();
        let mut src = Activity::own().data_source();
        let c2p_sem = Semaphore::bind(src.pop().unwrap());
        let child_sel = src.pop().unwrap();

        let child_rgate = wv_require_ok!(RecvCap::new_with(RGateArgs::default().sel(child_sel)));
        wv_assert_ok!(t, c2p_sem.up());

        // wait a bit to let our parent start the sem down syscall
        OwnActivity::sleep_for(TimeDuration::from_millis(1)).unwrap();

        // revoke the rgate
        drop(child_rgate);
        Ok(())
    }));

    wv_assert_ok!(t, c2p_sem.down());

    let our_sel = wv_require_ok!(act.activity().obtain_obj(child_sel));
    let child_rgate = RecvCap::new_bind(our_sel);
    wv_assert_err!(t, SendGate::new(&child_rgate), Code::ObjectGone);

    wv_assert_ok!(t, act.wait());
}
