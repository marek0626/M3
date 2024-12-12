/*
 * Copyright (C) 2022 Nils Asmussen, Barkhausen Institut
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

#![feature(io_error_more)]

extern crate m3core as m3;

#[allow(unused_extern_crates)]
extern crate m3files;

use m3::errors::Error;
use m3::test::{DefaultWvTester, WvTester};
use m3::wv_run_suite;

mod tdir;
mod tfile;
mod tsocket;
mod ttime;

use core::ptr;

extern "C" {
    fn __m3_init_libc(argc: i32, argv: *const *const u8, envp: *const *const u8, tls: bool);
}

// This env_run function is necessary because this crate does *not* use the
// normal, outward-facing m3 crate. That crate ties together the internals and
// provides the runtime initializers. Thus this crate must provide them
// itself.
#[no_mangle]
pub extern "C" fn env_run() -> ! {
    unsafe {
        __m3_init_libc(0, ptr::null(), ptr::null(), false);
    }

    m3files::vfs_init().expect("Couldn't init vfs subsystem.");
    m3core::env::init();

    m3core::env::run();
}

#[macro_export]
macro_rules! wv_assert_stderr {
    ($t:expr, $a:expr, $e:expr) => {{
        m3::wv_assert!($t, matches!($a, Err(e) if e.kind() == $e));
    }};
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, tdir::run);
    wv_run_suite!(tester, tfile::run);
    wv_run_suite!(tester, tsocket::run);
    wv_run_suite!(tester, ttime::run);
    println!("{}", tester);
    Ok(())
}
