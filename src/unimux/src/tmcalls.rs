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

use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{GlobAddr, GlobAddrRaw, VirtAddr};
use base::tcu::{EpId, INVALID_EP, IRQ};
use base::time::TimeDuration;
use base::tmif;

use isr::{ISRArch, ISR};

use crate::{activities, arch, timer};
use mux::helper;

fn tmcall_stop(state: &mut arch::State) -> Result<(), Error> {
    let code = Code::try_from(state.r[isr::TMC_ARG1] as u32)?;

    log!(LogFlags::MuxCalls, "tmcall::stop(code={:?})", code);

    activities::stop_activity(code);

    Ok(())
}

fn tmcall_translate(state: &mut arch::State) -> Result<(), Error> {
    let virt = VirtAddr::from(state.r[isr::TMC_ARG1]);

    log!(LogFlags::MuxCalls, "tmcall::translate(virt={})", virt);

    state.r[isr::TMC_ARG1] = virt.as_raw() as usize;
    Ok(())
}

fn tmcall_wait(state: &mut arch::State) -> Result<(), Error> {
    let timeout = match state.r[isr::TMC_ARG4] {
        usize::MAX => None,
        t => Some(TimeDuration::from_nanos(t as u64)),
    };

    if let Some(t) = timeout {
        activities::user().set_blocked(true);
        timer::set_timeout(t);
        crate::reg_timer_reprogram();
    }
    Ok(())
}

pub fn handle_call(state: &mut arch::State) {
    let opcode = state.r[isr::TMC_ARG0];

    let res = match opcode {
        o if o == tmif::Operation::Exit.into() => tmcall_stop(state),
        o if o == tmif::Operation::Translate.into() => tmcall_translate(state),
        o if o == tmif::Operation::Wait.into() => tmcall_wait(state),
        _ => Err(Error::new(Code::InvArgs)),
    };

    if let Err(e) = &res {
        log!(
            LogFlags::MuxCalls,
            "\x1B[1mError for call {:?}: {:?}\x1B[0m",
            tmif::Operation::try_from(opcode),
            e.code()
        );
    }

    state.r[isr::TMC_ARG0] = match res {
        Ok(_) => 0,
        Err(e) => e.code() as usize,
    };
}
