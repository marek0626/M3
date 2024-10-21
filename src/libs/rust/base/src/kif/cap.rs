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

use core::fmt;

use num_enum::{FromPrimitive, IntoPrimitive};

use crate::{
    errors::{Code, Error},
    serialize::{Deserialize, Serialize},
};

/// A capability selector
pub type CapSel = u64;

/// A capability range descriptor, which describes a continuous range of capabilities
///
/// It is guaranteed that the last capability selector (if any) is not out of
/// bounds.
/// However, one past the last capability selector may overflow.
/// Furthermore, the range might be of zero size.
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[serde(try_from = "UnsafeCapRngDesc")]
pub struct CapRngDesc {
    start: u64,
    /// This is the count in the upper bits and the type in the lowest bit
    count_ty: u64,
}

/// The capability types
#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive, FromPrimitive)]
#[repr(u64)]
pub enum CapType {
    /// Object capabilities are used for kernel objects (SendGate, Activity, ...)
    #[default]
    Object,
    /// Mapping capabilities are used for page table entries
    Mapping,
}

impl CapRngDesc {
    /// Creates a new capability range descriptor. `start` is the first capability selector and
    /// `start + count - 1` is the last one.
    pub fn new(ty: CapType, start: CapSel, count: CapSel) -> Result<Self, Error> {
        // Check that count can be shifted left by one without changing the
        // value.
        let shifted = count
            .checked_mul(2)
            .ok_or_else(|| Error::new(Code::CapCountTooLarge))?;
        UnsafeCapRngDesc {
            start,
            count_ty: shifted | (ty as u64),
        }
        .try_into()
    }

    /// Create a new descriptor for a range of size one
    ///
    /// This construction cannot fail as such a range is always representable.
    pub fn new_single(ty: CapType, sel: CapSel) -> Self {
        // Should be optimized by compiler.
        Self::new(ty, sel, 1).unwrap()
    }

    /// Create a range descriptor without performing any bounds checking.
    ///
    /// # Safety
    ///
    /// The last element must still be representable.
    /// Beware that the count value is shifted left.
    /// This function is only intended for test cases where we want to test
    /// that the kernel checks the range descriptor on deserialization.
    pub unsafe fn new_unchecked(ty: CapType, start: CapSel, count: CapSel) -> Self {
        Self {
            start,
            count_ty: count << 1 | (ty as u64),
        }
    }

    /// Returns the capability type
    pub fn cap_type(self) -> CapType {
        CapType::from(self.count_ty & 0x1)
    }

    /// Returns the first capability selector
    pub fn start(self) -> CapSel {
        self.start
    }

    /// Returns the number of capability selectors
    pub fn count(self) -> CapSel {
        self.count_ty >> 1
    }
}

impl fmt::Display for CapRngDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CRD[{:?}: {}:{}]",
            self.cap_type(),
            self.start(),
            self.count()
        )
    }
}

/// Helper struct that binary data is deserialized into without validation
///
/// [`CapRngDesc`] is created from this after validation
#[derive(Deserialize)]
struct UnsafeCapRngDesc {
    start: u64,
    count_ty: u64,
}

impl TryFrom<UnsafeCapRngDesc> for CapRngDesc {
    type Error = Error;

    fn try_from(desc: UnsafeCapRngDesc) -> Result<Self, Self::Error> {
        let unval = CapRngDesc {
            start: desc.start,
            count_ty: desc.count_ty,
        };
        // Only when count > 0 overflows can occur.
        if let Some(c) = unval.count().checked_sub(1) {
            // Try to compute last element without overflow.
            let last = unval.start().checked_add(c);
            if last.is_none() {
                return Err(Error::new(Code::LastCapOverflow));
            }
        }
        // Guaranteed that the last capability selector is representable.
        Ok(unval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_rng_desc_single() {
        for sel in [0, 1, 1234, CapSel::MAX] {
            let single = CapRngDesc::new_single(CapType::Object, sel);
            assert_eq!(single.start(), sel);
            assert_eq!(single.count(), 1);
            assert_eq!(single.cap_type(), CapType::Object);
        }
    }

    #[test]
    fn cap_rng_desc_new() {
        // Test an ordinary range.
        let rng = CapRngDesc::new(CapType::Mapping, 4321, 6);
        assert!(rng.is_ok());
        if let Ok(rng) = rng {
            assert_eq!(rng.start(), 4321);
            assert_eq!(rng.count(), 6);
            assert_eq!(rng.cap_type(), CapType::Mapping);
        }

        // Test and empty range.
        let rng = CapRngDesc::new(CapType::Object, 4321, 0);
        assert!(rng.is_ok());
        if let Ok(rng) = rng {
            assert_eq!(rng.count(), 0);
        }

        // Test a range of maximum size.
        let rng = CapRngDesc::new(CapType::Object, 0, CapSel::MAX >> 1);
        assert!(rng.is_ok());
        if let Ok(rng) = rng {
            assert_eq!(rng.start(), 0);
            assert_eq!(rng.count(), CapSel::MAX >> 1);
            assert_eq!(rng.cap_type(), CapType::Object);
        }

        // Test a range at the end.
        let rng = CapRngDesc::new(CapType::Object, CapSel::MAX, 1);
        assert!(rng.is_ok());
        if let Ok(rng) = rng {
            assert_eq!(rng.start(), CapSel::MAX);
            assert_eq!(rng.count(), 1);
            assert_eq!(rng.cap_type(), CapType::Object);
        }

        // Test capability count.
        assert_eq!(
            CapRngDesc::new(CapType::Mapping, 1337, (CapSel::MAX >> 1) + 1)
                .map(|_| ())
                .map_err(|e| e.code()),
            Err(Code::CapCountTooLarge)
        );
        assert_eq!(
            CapRngDesc::new(CapType::Mapping, 0, CapSel::MAX)
                .map(|_| ())
                .map_err(|e| e.code()),
            Err(Code::CapCountTooLarge)
        );

        // Test overflow.
        assert_eq!(
            CapRngDesc::new(CapType::Object, CapSel::MAX, 2)
                .map(|_| ())
                .map_err(|e| e.code()),
            Err(Code::LastCapOverflow)
        );
    }
}
