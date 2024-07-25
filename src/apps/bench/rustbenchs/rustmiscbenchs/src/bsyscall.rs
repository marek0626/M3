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

use m3::cap::SelSpace;
use m3::cell::StaticCell;
use m3::cfg;
use m3::com::{EpMng, MemCap, MemGate, Perm, RecvCap, RecvGate};
use m3::kif;
use m3::mem::{GlobOff, VirtAddr};
use m3::rc::Rc;
use m3::syscalls;
use m3::test::WvTester;
use m3::tiles::{Activity, ActivityArgs, ChildActivity, Tile};
use m3::time::{CycleInstant, Profiler, Runner};
use m3::util::math;
use m3::vec::Vec;
use m3::{println, wv_assert_ok, wv_perf, wv_require_ok, wv_run_test};

static SEL: StaticCell<kif::CapSel> = StaticCell::new(0);

pub fn run(t: &mut dyn WvTester) {
    SEL.set(SelSpace::get().alloc_sel());

    wv_run_test!(t, noop);
    wv_run_test!(t, activate);
    wv_run_test!(t, create_mgate);
    wv_run_test!(t, create_rgate);
    wv_run_test!(t, create_sgate);
    wv_run_test!(t, create_map);
    wv_run_test!(t, create_srv);
    wv_run_test!(t, derive_mem);
    wv_run_test!(t, exchange);
    wv_run_test!(t, revoke_mem_gate);
    wv_run_test!(t, revoke_recv_gate);
    wv_run_test!(t, revoke_send_gate);
    wv_run_test!(t, revoke_deep);
    wv_run_test!(t, revoke_wide);
}

fn noop(t: &mut dyn WvTester) {
    let prof = Profiler::default();

    wv_perf!(
        "noop",
        prof.run::<CycleInstant, _>(|| {
            wv_assert_ok!(t, syscalls::noop());
        })
    );
}

fn activate(t: &mut dyn WvTester) {
    let mcap = wv_require_ok!(MemCap::new(0x1000, Perm::RW));
    let ep = wv_require_ok!(EpMng::get().acquire(0));

    let prof = Profiler::default();

    wv_perf!(
        "activate",
        prof.run::<CycleInstant, _>(|| {
            wv_assert_ok!(t, syscalls::activate_mgate(ep.sel(), mcap.sel()));
        })
    );

    EpMng::get().release(ep, true);
}

fn create_mgate(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(100);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        virt: VirtAddr,
    }

    impl Runner for Tester<'_> {
        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::create_mgate(
                    SEL.get(),
                    Activity::own().sel(),
                    self.virt,
                    cfg::PAGE_SIZE as GlobOff,
                    Perm::R
                )
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    let addr = VirtAddr::from(math::round_dn(
        create_mgate as *const () as usize,
        cfg::PAGE_SIZE,
    ));
    wv_perf!(
        "create_mgate",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            virt: addr
        })
    );
}

fn create_rgate(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(100);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
    }

    impl Runner for Tester<'_> {
        fn run(&mut self) {
            wv_assert_ok!(self.tester, syscalls::create_rgate(SEL.get(), 10, 10));
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "create_rgate",
        prof.runner::<CycleInstant, _>(&mut Tester { tester: t })
    );
}

fn create_sgate(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        rgate: Option<RecvGate>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            if self.rgate.is_none() {
                self.rgate = Some(wv_require_ok!(RecvGate::new(10, 10)));
            }
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::create_sgate(SEL.get(), self.rgate.as_ref().unwrap().sel(), 0x1234, 1024)
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "create_sgate",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            rgate: None,
        })
    );
}

fn create_map(t: &mut dyn WvTester) {
    if !Activity::own().tile_desc().has_virtmem() {
        println!("Tile has no virtual memory support; skipping");
        return;
    }

    const DEST: VirtAddr = VirtAddr::new(0x3000_0000);
    let prof = Profiler::default().repeats(25).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        mgate: MemGate,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            // one warmup run, because the revoke leads to an unmap, which flushes and invalidates
            // all cache lines
            wv_assert_ok!(
                self.tester,
                syscalls::create_map(
                    DEST,
                    Activity::own().sel(),
                    self.mgate.sel(),
                    0,
                    1,
                    Perm::RW
                )
            );
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::create_map(
                    DEST + cfg::PAGE_SIZE,
                    Activity::own().sel(),
                    self.mgate.sel(),
                    1,
                    1,
                    Perm::RW
                )
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new(
                        kif::CapType::Mapping,
                        DEST.as_goff() / cfg::PAGE_SIZE as GlobOff,
                        2
                    )
                    .unwrap(),
                    true
                )
            );
        }
    }

    let mut tester = Tester {
        tester: t,
        mgate: MemGate::new((cfg::PAGE_SIZE * 2) as GlobOff, Perm::RW).unwrap(),
    };
    wv_perf!("create_map", prof.runner::<CycleInstant, _>(&mut tester));
}

fn create_srv(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        rgate: Option<RecvGate>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            if self.rgate.is_none() {
                self.rgate = Some(wv_require_ok!(RecvGate::new(10, 10)));
            }
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::create_srv(SEL.get(), self.rgate.as_ref().unwrap().sel(), "test", 0)
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "create_srv",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            rgate: None,
        })
    );
}

fn derive_mem(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        mgate: Option<MemGate>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            if self.mgate.is_none() {
                self.mgate = Some(wv_require_ok!(MemGate::new(0x1000, Perm::RW)));
            }
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::derive_mem(
                    Activity::own().sel(),
                    SEL.get(),
                    self.mgate.as_ref().unwrap().sel(),
                    0,
                    0x1000,
                    Perm::RW
                )
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "derive_mem",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            mgate: None,
        })
    );
}

fn exchange(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        act: Option<ChildActivity>,
        tile: Rc<Tile>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            if self.act.is_none() {
                self.act = Some(wv_require_ok!(ChildActivity::new_with(
                    self.tile.clone(),
                    ActivityArgs::new("test")
                )));
            }
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::exchange(
                    self.act.as_ref().unwrap().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, kif::SEL_ACT),
                    SEL.get(),
                    false,
                )
            );
        }

        fn post(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    self.act.as_ref().unwrap().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "exchange",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            act: None,
            tile: wv_require_ok!(Tile::get("compat|own")),
        })
    );
}

fn revoke_mem_gate(_t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    let mcap = wv_require_ok!(MemCap::new(0x1000, Perm::RW));

    struct Tester {
        mcap: MemCap,
        _derived: Option<MemCap>,
    }

    impl Runner for Tester {
        fn pre(&mut self) {
            self._derived = Some(wv_require_ok!(self.mcap.derive(0, 0x1000, Perm::RW)));
        }

        fn run(&mut self) {
            self._derived = None;
        }
    }

    let mut tester = Tester {
        mcap,
        _derived: None,
    };
    wv_perf!(
        "revoke_mem_gate",
        prof.runner::<CycleInstant, _>(&mut tester)
    );
}

fn revoke_recv_gate(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            wv_assert_ok!(self.tester, syscalls::create_rgate(SEL.get(), 10, 10));
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "revoke_recv_gate",
        prof.runner::<CycleInstant, _>(&mut Tester { tester: t })
    );
}

fn revoke_send_gate(t: &mut dyn WvTester) {
    let prof = Profiler::default().repeats(100).warmup(10);

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        rcap: Option<RecvCap>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            self.rcap = Some(wv_require_ok!(RecvCap::new(10, 10)));
            wv_assert_ok!(
                self.tester,
                syscalls::create_sgate(SEL.get(), self.rcap.as_ref().unwrap().sel(), 0x1234, 1024)
            );
        }

        fn run(&mut self) {
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(
                    Activity::own().sel(),
                    kif::CapRngDesc::new_single(kif::CapType::Object, SEL.get()),
                    true
                )
            );
        }
    }

    wv_perf!(
        "revoke_send_gate",
        prof.runner::<CycleInstant, _>(&mut Tester {
            tester: t,
            rcap: None,
        })
    );
}

/// Test performance of revoke on a deep derivation tree.
fn revoke_deep(t: &mut dyn WvTester) {
    const SIZE: u64 = 0x1000;
    const PERM: Perm = Perm::RW;
    const DEPTH: usize = 16;

    let prof = Profiler::default().repeats(100).warmup(2);

    let mcap = wv_require_ok!(MemCap::new(SIZE, PERM));

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        mcap: MemCap,
        _derived: Vec<MemCap>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            // Drop capabilities outside run().
            self._derived.clear();

            // Create a deep branch in the derivation tree.
            self._derived.push(MemCap::new_bind(self.mcap.sel()));
            for _ in 0..DEPTH {
                let mem = wv_require_ok!(self._derived.last().unwrap().derive(0, SIZE, PERM));
                // Keep capability around to avoid revocation.
                self._derived.push(mem);
            }
        }

        fn run(&mut self) {
            let crd = kif::CapRngDesc::new_single(kif::CapType::Object, self.mcap.sel());
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(Activity::own().sel(), crd, false)
            );
        }
    }

    let mut tester = Tester {
        tester: t,
        mcap,
        _derived: Vec::with_capacity(DEPTH),
    };
    wv_perf!("revoke_deep", prof.runner::<CycleInstant, _>(&mut tester));
}

/// Test performance of revoke on a wide derivation tree.
fn revoke_wide(t: &mut dyn WvTester) {
    const SIZE: u64 = 0x1000;
    const PERM: Perm = Perm::RW;
    const WIDTH: usize = 64;

    let prof = Profiler::default().repeats(50).warmup(2);

    let mcap = wv_require_ok!(MemCap::new(SIZE, PERM));

    struct Tester<'a> {
        tester: &'a mut dyn WvTester,
        mcap: MemCap,
        _derived: Vec<MemCap>,
    }

    impl Runner for Tester<'_> {
        fn pre(&mut self) {
            // Drop capabilities outside run().
            self._derived.clear();

            // Create a wide sibling structure in the derivation tree.
            for _ in 0..WIDTH {
                let mem = wv_require_ok!(self.mcap.derive(0, SIZE, PERM));
                // Keep capability around to avoid revocation.
                self._derived.push(mem);
            }
        }

        fn run(&mut self) {
            let crd = kif::CapRngDesc::new_single(kif::CapType::Object, self.mcap.sel());
            wv_assert_ok!(
                self.tester,
                syscalls::revoke(Activity::own().sel(), crd, false)
            );
        }
    }

    let mut tester = Tester {
        tester: t,
        mcap,
        _derived: Vec::with_capacity(WIDTH),
    };
    wv_perf!("revoke_wide", prof.runner::<CycleInstant, _>(&mut tester));
}
