/*
 * Copyright (C) 2020-2022 Nils Asmussen, Barkhausen Institut
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

use base::cell::StaticCell;
use base::io::LogFlags;
use base::log;
use base::tcu;
use base::time::TimeDuration;

use crate::hdl::entry;

static TIMEOUT: StaticCell<TimeDuration> = StaticCell::new(TimeDuration::ZERO);

pub fn set_timeout(time: TimeDuration) {
    TIMEOUT.set(time);
}

pub fn get_timeout() -> TimeDuration {
    TIMEOUT.replace(TimeDuration::ZERO)
}

// this function should only be called from the root module; others can request it by calling
// crate::trigger().
pub fn reprogram() {
    let timeout = get_timeout();
    log!(LogFlags::MuxTimer, "timer: setting timer to {:?}", timeout);
    tcu::TCU::set_timer(timeout.as_nanos() as u64).unwrap();
}

pub fn trigger() {
    entry::reg_timer_reprogram();
}
