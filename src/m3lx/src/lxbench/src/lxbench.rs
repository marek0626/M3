/*
 * Copyright (C) 2022-2023 Oliver Portee <oliver.portee@gmail.com>
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

use m3::{
    test::{DefaultWvTester, WvTester},
    vfs::VFS,
};

mod bmisc;
mod bregfile;

fn main() -> Result<(), std::io::Error> {
    m3::env::init();

    VFS::mount("/", "m3fs", "m3fs").unwrap();

    let mut tester = DefaultWvTester::default();
    m3::wv_run_suite!(tester, bregfile::run);
    m3::wv_run_suite!(tester, bmisc::run);
    println!("{}", tester);
    Ok(())
}
