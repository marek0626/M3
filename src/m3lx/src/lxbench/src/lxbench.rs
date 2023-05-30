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

use util::mmap::{MemType, Mmap};

use m3::{
    cfg,
    linux::ioctl,
    tcu,
    test::{DefaultWvTester, WvTester},
    vfs::VFS,
};

mod bmisc;
mod bregfile;

fn main() -> Result<(), std::io::Error> {
    ioctl::init();

    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, MemType::TCU, tcu::MMIO_SIZE)?;

    let _env_mmap = Mmap::new(
        "/dev/tcu",
        cfg::ENV_START,
        MemType::Environment,
        cfg::ENV_SIZE,
    )?;
    let env = m3::env::get();

    let (rbuf_virt_addr, rbuf_size) = env.tile_desc().rbuf_std_space();
    let _rcv_mmap = Mmap::new("/dev/tcu", rbuf_virt_addr, MemType::StdRecvBuf, rbuf_size)?;

    // m3 setup
    m3::env::init();

    VFS::mount("/", "m3fs", "m3fs").unwrap();

    let mut tester = DefaultWvTester::default();
    m3::wv_run_suite!(tester, bregfile::run);
    m3::wv_run_suite!(tester, bmisc::run);
    println!("{}", tester);
    Ok(())
}
