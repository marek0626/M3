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

#![no_std]

mod helper;
mod tchilds;
mod tmemory;
mod tparse;
mod tsubsys;
mod ttiles;
mod tvalidator;

use m3::errors::Error;
use m3::println;
use m3::test::{DefaultWvTester, WvTester};
use m3::wv_run_suite;

#[macro_export]
macro_rules! wv_assert_anyhow_err {
    ($t:expr, $res:expr, $err:expr) => {{
        let res = $res;
        match res {
            Ok(r) => {
                ::m3::println!("! {}:{}  received okay: {:?} FAILED", file!(), line!(), r);
                $t.test_failed();
            },
            Err(ref e) if e.downcast_ref::<m3::errors::Error>().unwrap().code() != $err => {
                ::m3::println!(
                    "! {}:{}  received error {:?}, expected {:?} FAILED",
                    file!(),
                    line!(),
                    e,
                    $err
                );
                $t.test_failed();
            },
            Err(_) => {
                $t.test_succeeded();
            },
        }
    }};
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, tchilds::run);
    wv_run_suite!(tester, tmemory::run);
    wv_run_suite!(tester, tparse::run);
    wv_run_suite!(tester, tsubsys::run);
    wv_run_suite!(tester, ttiles::run);
    wv_run_suite!(tester, tvalidator::run);
    println!("{}", tester);
    Ok(())
}
