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

use m3::errors::Code;
use m3::test::WvTester;
use m3::tiles::{ActivityArgs, ChildActivity, RunningActivity, Tile};
use m3::{wv_assert_eq, wv_assert_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, act_exec);
    wv_run_test!(t, act_run);
}

fn act_exec(t: &mut dyn WvTester) {
    let tile = wv_assert_ok!(Tile::get("own"));
    let act = wv_assert_ok!(ChildActivity::new_with(
        tile.clone(),
        ActivityArgs::new("test")
    ));
    let act = wv_assert_ok!(act.exec(&["/bin/ps"]));
    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}

fn act_run(t: &mut dyn WvTester) {
    let tile = wv_assert_ok!(Tile::get("own"));
    let act = wv_assert_ok!(ChildActivity::new_with(tile, ActivityArgs::new("test")));
    let act = wv_assert_ok!(act.run(|| {
        println!("Hello World!");
        Ok(())
    }));
    wv_assert_eq!(t, act.wait(), Ok(Code::Success));
}
