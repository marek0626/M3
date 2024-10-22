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
use m3::com::{recv_msg, RecvGate, SGateArgs, SendCap, SendGate};
use m3::errors::Code;
use m3::kif::{CapRngDesc, CapType};
use m3::test::{DefaultWvTester, WvTester};
use m3::tiles::{Activity, ActivityArgs, ChildActivity, OwnActivity, RunningActivity, Tile};
use m3::time::TimeDuration;

use m3::{send_vmsg, syscalls, wv_assert_err, wv_assert_ok, wv_require_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, run_stop);
    wv_run_test!(t, kmem_revoke);
    wv_run_test!(t, tile_revoke);
}

fn run_stop(t: &mut dyn WvTester) {
    use m3::com::RGateArgs;
    use m3::vfs;

    let rg = wv_require_ok!(RecvGate::new_with(
        RGateArgs::default().order(6).msg_order(6)
    ));

    let tile = wv_require_ok!(Tile::get("compat|own"));

    let mut wait_time = TimeDuration::from_nanos(10000);
    for _ in 1..100 {
        let mut act = wv_require_ok!(ChildActivity::new_with(
            tile.clone(),
            ActivityArgs::new("test")
        ));

        // pass sendgate to child
        let sg = wv_require_ok!(SendCap::new_with(SGateArgs::new(&rg).credits(1)));
        wv_assert_ok!(t, act.delegate_obj(sg.sel()));

        // pass root fs to child
        act.add_mount("/", "/");

        let mut dst = act.data_sink();
        dst.push(sg.sel());

        let act = wv_require_ok!(act.run(|| {
            let mut t = DefaultWvTester::default();

            let mut src = Activity::own().data_source();
            let sg_sel: Selector = src.pop().unwrap();

            // notify parent that we're running
            let sg = wv_require_ok!(SendGate::new_bind(sg_sel));
            wv_assert_ok!(t, send_vmsg!(&sg, RecvGate::def(), 1));
            let mut _n = 0;
            loop {
                _n += 1;
                // just to execute more interesting instructions than arithmetic or jumps
                vfs::VFS::stat("/").ok();
            }
        }));

        // wait for child
        wv_assert_ok!(t, recv_msg(&rg));

        // wait a bit and stop activity
        wv_assert_ok!(t, OwnActivity::sleep_for(wait_time));
        wv_assert_ok!(t, act.stop());

        // increase by one ns to attempt interrupts at many points in the instruction stream
        wait_time += TimeDuration::from_nanos(1);
    }
}

fn kmem_revoke(t: &mut dyn WvTester) {
    let own_kmem = Activity::own().kmem();
    let cur_quota = wv_require_ok!(own_kmem.quota());
    let child_kmem = wv_require_ok!(own_kmem.derive(cur_quota.remaining() / 2));

    let tile = wv_require_ok!(Tile::get("compat|own"));
    let act = wv_require_ok!(ChildActivity::new_with(
        tile.clone(),
        ActivityArgs::new("test").kmem(child_kmem.clone())
    ));

    // revoke kernel memory to also revoke the activity
    wv_assert_ok!(
        t,
        Activity::own().revoke(
            CapRngDesc::new_single(CapType::Object, child_kmem.sel()),
            false
        )
    );

    wv_assert_err!(t, syscalls::activity_wait(&[act.sel()], 0), Code::InvCap);
}

fn tile_revoke(t: &mut dyn WvTester) {
    let tile = wv_require_ok!(Tile::get("compat|own"));
    let child_tile = wv_require_ok!(tile.derive(None, None, None, None));
    let act = wv_require_ok!(ChildActivity::new_with(
        child_tile.clone(),
        ActivityArgs::new("test")
    ));

    // revoke tile to also revoke the activity
    wv_assert_ok!(
        t,
        Activity::own().revoke(
            CapRngDesc::new_single(CapType::Object, child_tile.sel()),
            false
        )
    );

    wv_assert_err!(t, syscalls::activity_wait(&[act.sel()], 0), Code::InvCap);
}
