/*
 * Copyright (C) 2o24 Viktor Reusch, Barkhausen Institut
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
use m3::kif::{CapRngDesc, CapSel, CapType};
use m3::test::WvTester;
use m3::{wv_assert_eq, wv_assert_err, wv_assert_ok, wv_run_test};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, cap_rng_desc_single);
    wv_run_test!(t, cap_rng_desc_new);
}

fn cap_rng_desc_single(t: &mut dyn WvTester) {
    for sel in [0, 1, 1234, CapSel::MAX] {
        let single = CapRngDesc::new_single(CapType::Object, sel);
        wv_assert_eq!(t, single.start(), sel);
        wv_assert_eq!(t, single.count(), 1);
        wv_assert_eq!(t, single.cap_type(), CapType::Object);
    }
}

fn cap_rng_desc_new(t: &mut dyn WvTester) {
    // Test an ordinary range.
    let rng = CapRngDesc::new(CapType::Mapping, 4321, 6);
    wv_assert_ok!(t, &rng);
    if let Ok(rng) = rng {
        wv_assert_eq!(t, rng.start(), 4321);
        wv_assert_eq!(t, rng.count(), 6);
        wv_assert_eq!(t, rng.cap_type(), CapType::Mapping);
    }

    // Test and empty range.
    let rng = CapRngDesc::new(CapType::Object, 4321, 0);
    wv_assert_ok!(t, &rng);
    if let Ok(rng) = rng {
        wv_assert_eq!(t, rng.count(), 0);
    }

    // Test a range of maximum size.
    let rng = CapRngDesc::new(CapType::Object, 0, CapSel::MAX >> 1);
    wv_assert_ok!(t, &rng);
    if let Ok(rng) = rng {
        wv_assert_eq!(t, rng.start(), 0);
        wv_assert_eq!(t, rng.count(), CapSel::MAX >> 1);
        wv_assert_eq!(t, rng.cap_type(), CapType::Object);
    }

    // Test a range at the end.
    let rng = CapRngDesc::new(CapType::Object, CapSel::MAX, 1);
    wv_assert_ok!(t, &rng);
    if let Ok(rng) = rng {
        wv_assert_eq!(t, rng.start(), CapSel::MAX);
        wv_assert_eq!(t, rng.count(), 1);
        wv_assert_eq!(t, rng.cap_type(), CapType::Object);
    }

    // Test capability count.
    wv_assert_err!(
        t,
        CapRngDesc::new(CapType::Mapping, 1337, (CapSel::MAX >> 1) + 1),
        Code::CapCountTooLarge
    );
    wv_assert_err!(
        t,
        CapRngDesc::new(CapType::Mapping, 0, CapSel::MAX),
        Code::CapCountTooLarge
    );

    // Test overflow.
    wv_assert_err!(
        t,
        CapRngDesc::new(CapType::Object, CapSel::MAX, 2),
        Code::LastCapOverflow
    );
}
