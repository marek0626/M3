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

mod tbufio;
mod tdir;
mod tfilemux;
mod tgenfile;
mod tm3fs;
mod tnonblock;
mod tpipe;

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, tbufio::run);
    wv_run_suite!(tester, tdir::run);
    wv_run_suite!(tester, tfilemux::run);
    wv_run_suite!(tester, tgenfile::run);
    wv_run_suite!(tester, tm3fs::run);
    wv_run_suite!(tester, tnonblock::run);
    wv_run_suite!(tester, tpipe::run);
    println!("{}", tester);
    Ok(())
}
