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
use m3::com::Semaphore;
use m3::errors::Code;
use m3::test::{DefaultWvTester, WvTester};
use m3::tiles::{Activity, ActivityArgs, ChildActivity, OwnActivity, RunningActivity, Tile};
use m3::time::TimeDuration;
use m3::{wv_assert_err, wv_assert_ok, wv_require_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, destroy);
}

fn destroy(t: &mut dyn WvTester) {
    let sig_sem = wv_require_ok!(Semaphore::create(0));
    let child_sel = SelSpace::get().alloc_sel();

    let tile = wv_require_ok!(Tile::get("compat|own"));
    let mut child = wv_require_ok!(ChildActivity::new_with(
        tile,
        // ensure that child_sel is not reused by the child
        ActivityArgs::new("child").first_sel(child_sel + 1)
    ));
    wv_assert_ok!(t, child.delegate_obj(sig_sem.sel()));

    let mut dst = child.data_sink();
    dst.push(sig_sem.sel());
    dst.push(child_sel);

    let act = wv_require_ok!(child.run(|| {
        let mut t = DefaultWvTester::default();
        let mut src = Activity::own().data_source();
        let sig_sem = Semaphore::bind(src.pop().unwrap());
        let child_sel = src.pop().unwrap();

        let child_sem = wv_require_ok!(Semaphore::create_with_sel(0, child_sel));
        wv_assert_ok!(t, sig_sem.up());

        // wait a bit to let our parent start the sem down syscall
        OwnActivity::sleep_for(TimeDuration::from_millis(1)).unwrap();

        // now revoke the semaphore
        drop(child_sem);
        Ok(())
    }));

    wv_assert_ok!(t, sig_sem.down());

    let our_sel = wv_require_ok!(act.activity().obtain_obj(child_sel));
    let child_sem = Semaphore::bind(our_sel);

    wv_assert_err!(t, child_sem.down(), Code::ObjectGone);
    wv_assert_ok!(t, act.wait());
}
