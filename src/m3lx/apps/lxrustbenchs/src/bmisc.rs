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

use m3::{
    build_vmsg,
    errors::{Code, Error},
    io::{Read, Write},
    kif::{self, Perm},
    linux::ioctl,
    mem::{MsgBuf, VirtAddr},
    serialize::M3Deserializer,
    tcu::{self, EpId},
    test::WvTester,
    tiles::Activity,
    time::{CycleInstant, Profiler, Runner},
    vfs::{FileMode, FileRef, GenericFile, OpenFlags, VFS},
    wv_assert_ok, wv_perf, wv_run_test,
};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, bench_m3fs_meta);
    wv_run_test!(t, bench_custom_noop_syscall);
    wv_run_test!(t, bench_m3_noop_syscall);
    wv_run_test!(t, bench_os_call);
    wv_run_test!(t, bench_tlb_insert);
    wv_run_test!(t, bench_m3fs_read);
    wv_run_test!(t, bench_m3fs_write);
}

fn wait_for_rpl(rep: EpId, rcv_buf: VirtAddr) -> Result<(), Error> {
    loop {
        if let Some(off) = tcu::TCU::fetch_msg(rep) {
            let msg = tcu::TCU::offset_to_msg(rcv_buf, off);
            let mut de = M3Deserializer::new(msg.as_words());
            let res: Code = de.pop()?;
            tcu::TCU::ack_msg(rep, off)?;
            return match res {
                Code::Success => Ok(()),
                c => Err(Error::new(c)),
            };
        }
    }
}

fn noop_syscall(rbuf: VirtAddr) -> Result<(), Error> {
    let mut msg = MsgBuf::borrow_def();
    build_vmsg!(msg, kif::syscalls::Operation::Noop, kif::syscalls::Noop {});
    tcu::TCU::send(
        tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
        &msg,
        0,
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
    )?;
    wait_for_rpl(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, rbuf)
}

#[inline(never)]
fn bench_custom_noop_syscall(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    let (rbuf, _) = Activity::own().tile_desc().rbuf_std_space();
    wv_perf!(
        "custom-noop-syscall",
        profiler.run::<CycleInstant, _>(|| {
            wv_assert_ok!(noop_syscall(rbuf));
        })
    );
}

#[inline(never)]
fn bench_m3_noop_syscall(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    wv_perf!(
        "noop-syscall",
        profiler.run::<CycleInstant, _>(|| {
            wv_assert_ok!(m3::syscalls::noop());
        })
    );
}

#[inline(never)]
fn bench_tlb_insert(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);
    let sample_addr = VirtAddr::from(&profiler as *const _);

    wv_perf!(
        "tlb-insert",
        profiler.run::<CycleInstant, _>(|| {
            tcu::TCU::handle_xlate_fault(sample_addr, Perm::R);
        })
    );
}

#[inline(never)]
fn bench_os_call(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    wv_perf!(
        "os-call",
        profiler.run::<CycleInstant, _>(|| {
            ioctl::noop();
        })
    );
}

const READ_STR_LEN: usize = 1024 * 1024;
const WRITE_STR_LEN: usize = 8 * 1024;

#[inline(never)]
fn bench_m3fs_read(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    let mut file = wv_assert_ok!(VFS::open(
        "/new-file.txt",
        OpenFlags::CREATE | OpenFlags::RW
    ));
    let content: String = (0..READ_STR_LEN).map(|_| "a").collect();
    wv_assert_ok!(write!(file, "{}", content));

    wv_perf!(
        "m3fs-read",
        profiler.run::<CycleInstant, _>(|| {
            let _content = wv_assert_ok!(file.read_to_string());
        })
    );

    VFS::unlink("/new-file.txt").unwrap();
}

struct WriteBenchmark {
    file: FileRef<GenericFile>,
    content: String,
}

impl WriteBenchmark {
    fn new() -> WriteBenchmark {
        WriteBenchmark {
            file: wv_assert_ok!(VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::W)),
            content: (0..WRITE_STR_LEN).map(|_| "a").collect(),
        }
    }
}

impl Drop for WriteBenchmark {
    fn drop(&mut self) {
        wv_assert_ok!(VFS::unlink("/new-file.txt"));
    }
}

impl Runner for WriteBenchmark {
    fn run(&mut self) {
        wv_assert_ok!(self.file.write_all(self.content.as_bytes()));
    }

    fn post(&mut self) {
        wv_assert_ok!(self.file.borrow().truncate(0));
    }
}

#[inline(never)]
fn bench_m3fs_write(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    wv_perf!(
        "m3fs-write",
        profiler.runner::<CycleInstant, _>(&mut WriteBenchmark::new())
    );
}

#[inline(never)]
fn bench_m3fs_meta(_t: &mut dyn WvTester) {
    let profiler = Profiler::default().warmup(10).repeats(100);

    wv_perf!(
        "m3fs-meta",
        profiler.run::<CycleInstant, _>(|| {
            wv_assert_ok!(VFS::mkdir("/new-dir", FileMode::from_bits(0o755).unwrap()));
            wv_assert_ok!(VFS::stat("/new-dir"));
            wv_assert_ok!(VFS::open("/new-dir/new-file", OpenFlags::CREATE));

            {
                let mut file = wv_assert_ok!(VFS::open("/new-dir/new-file", OpenFlags::W));
                wv_assert_ok!(write!(file, "test"));
            }

            {
                let mut file = wv_assert_ok!(VFS::open("/new-dir/new-file", OpenFlags::R));
                wv_assert_ok!(file.read_to_string());
                wv_assert_ok!(VFS::stat("/new-dir/new-file"));
            }

            wv_assert_ok!(VFS::link("/new-dir/new-file", "/new-link"));
            wv_assert_ok!(VFS::rename("/new-link", "/new-blink"));
            wv_assert_ok!(VFS::stat("/new-blink"));
            wv_assert_ok!(VFS::unlink("/new-blink"));
            wv_assert_ok!(VFS::unlink("/new-dir/new-file"));
            wv_assert_ok!(VFS::rmdir("/new-dir"));
        })
    );
}
