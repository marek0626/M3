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

use core::ops;

use base::errors::Code;
use base::mem::{PhysAddr, VirtAddr};
use base::tcu::TCU;
use base::tmif;

use crate::cap::{CapFlags, Capability, Selector};
use crate::com::{EpMng, EP};
use crate::errors::Error;
use crate::mem::GlobOff;
use crate::syscalls;
use crate::tcu::INVALID_EP;
use crate::{env, kif};

/// Represents a gate capability that can be turned into a usable gate (e.g., `SendCap` to
/// `SendGate`).
pub trait GateCap {
    /// The source type to construct a gate
    type Source;

    /// The target type for `activate` (e.g., `SendGate`)
    type Target;

    /// Creates a new instance for the given source
    fn new_from_cap(src: Self::Source) -> Self;

    /// Activates this `GateCap` and thereby turns it into a usable gate
    fn activate(self) -> Result<Self::Target, Error>;
}

/// A lazily activated gate
///
/// This type exists in two states: unactivated and activated. It can be used via `LazyGate::get`,
/// which will first activate it if not already done and return a usable gate.
///
/// Lazy activation is normally not necessary and also not desired as it comes with some overhead.
/// However, in some cases a gate needs to be activated lazily, i.e., on first use. For example, if
/// the gate is obtained from somebody else we cannot activate it immediately as the capability does
/// not exist until the obtain operation is finished.
#[derive(Debug)]
pub enum LazyGate<T: GateCap> {
    Unact(T::Source),
    Act(T::Target),
}

impl<T: GateCap> LazyGate<T> {
    /// Creates a new `LazyGate` with given selector
    pub fn new(src: T::Source) -> Self {
        Self::Unact(src)
    }

    /// Returns true if this `LazyGate` is already activated
    pub fn activated(&self) -> bool {
        matches!(self, Self::Act(_))
    }

    /// Requests access to the gate and returns a reference to it
    ///
    /// If not already done, this call will activate the gate.
    pub fn get(&mut self) -> Result<&T::Target, Error>
    where
        T::Source: Copy,
    {
        if let Self::Unact(src) = *self {
            *self = Self::Act(T::new_from_cap(src).activate()?);
        }

        match self {
            Self::Act(sg) => Ok(sg),
            _ => unreachable!(),
        }
    }
}

/// A gate is one side of a TCU-based communication channel and exists in the variants
/// [`MemGate`](`crate::com::MemGate`), [`SendGate`](`crate::com::SendGate`), and
/// [`RecvGate`](`crate::com::RecvGate`).
pub struct Gate {
    cap: Capability,
    ep: EP,
}

impl Gate {
    /// Creates a new gate with given capability selector and flags
    ///
    /// If ep is `None`, a new endpoint will be allocated, otherwise the given endpoint will be
    /// used. In either case, the gate will be activated on the endpoint.
    pub fn new(sel: Selector, flags: CapFlags, ep: Option<EP>, mem: bool) -> Result<Self, Error> {
        let ep = match ep {
            Some(ep) => ep,
            None => EpMng::get().acquire(0, false)?,
        };
        if mem {
            syscalls::activate_mgate(ep.sel(), sel)?;
        }
        else {
            syscalls::activate_sgate(ep.sel(), sel)?;
        }
        if TCU::is_frozen(ep.id()) {
            // nothing to check here as our integrity/confidentiality is not in danger
            TCU::unfreeze(ep.id())?;
        }
        Ok(Self::new_with_ep(sel, flags, ep))
    }

    /// Creates a new receive gate with given capability selector and flags and activates it on
    /// given EP
    pub fn new_rgate(
        sel: Selector,
        flags: CapFlags,
        mem: Option<Selector>,
        virt: VirtAddr,
        off: GlobOff,
        size: usize,
        ep: EP,
    ) -> Result<Self, Error> {
        syscalls::activate_rgate(ep.sel(), sel, mem.unwrap_or(kif::INVALID_SEL), off)?;
        if TCU::is_frozen(ep.id()) {
            let phys = tmif::translate(virt)?;
            let rinfo = TCU::recv_info(ep.id()).ok_or_else(|| Error::new(Code::KernelBroken))?;
            // check if the physical address and the buffer size is as expected (otherwise the
            // kernel could send us messages to overwrite specific areas of memory).
            if PhysAddr::new_raw(env::boot().tile_desc(), rinfo.0) != phys {
                return Err(Error::new(Code::KernelBroken));
            }
            if (1 << (rinfo.1 + rinfo.2)) as usize != size {
                return Err(Error::new(Code::KernelBroken));
            }
            // check that the reply EPs are at the expected position (otherwise the kernel could
            // let the TCU overwrite other send EPs and thereby trick us to send to unexpected
            // receivers).
            if rinfo.3 != ep.id() + 1 {
                return Err(Error::new(Code::KernelBroken));
            }
            TCU::unfreeze(ep.id())?;
        }
        Ok(Self::new_with_ep(sel, flags, ep))
    }

    /// Creates a new gate with given capability selector, flags, and endpoint
    pub const fn new_with_ep(sel: Selector, flags: CapFlags, ep: EP) -> Self {
        Gate {
            cap: Capability::new(sel, flags),
            ep,
        }
    }

    /// Returns the capability selector
    pub fn sel(&self) -> Selector {
        self.cap.sel()
    }

    /// Returns the flags that determine whether the capability will be revoked on destruction
    pub fn flags(&self) -> CapFlags {
        self.cap.flags()
    }

    pub(crate) fn set_flags(&mut self, flags: CapFlags) {
        self.cap.set_flags(flags);
    }

    pub(crate) fn ep(&self) -> &EP {
        &self.ep
    }

    pub(crate) fn release(&mut self, force_inval: bool) {
        // the destructing move sets the ep id to invalid to ensure that we release the EP just once
        if self.ep.id() != INVALID_EP {
            let ep = self.ep.destructing_move();
            EpMng::get().release(
                ep,
                force_inval || self.cap.flags().contains(CapFlags::KEEP_CAP),
            );
        }
    }
}

impl ops::Drop for Gate {
    fn drop(&mut self) {
        self.release(false);
    }
}
