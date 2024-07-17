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

use m3::errors::Error;
use m3::test::{DefaultWvTester, WvTester};
use m3::{println, wv_run_suite};

mod tboxlist;
mod tcap;
mod tdlist;
mod tenvvars;
mod tfloat;
mod tmemmap;
mod tserialize;
mod ttreap;

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, tboxlist::run);
    wv_run_suite!(tester, tdlist::run);
    wv_run_suite!(tester, tenvvars::run);
    wv_run_suite!(tester, tfloat::run);
    wv_run_suite!(tester, tmemmap::run);
    wv_run_suite!(tester, tserialize::run);
    wv_run_suite!(tester, ttreap::run);
    wv_run_suite!(tester, tcap::run);
    println!("{}", tester);
    Ok(())
}
