/*
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
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

//! Contains math functions

use num_traits::PrimInt;

use crate::mem;

/// Computes the square root of `val`.
///
/// Source: [Wikipedia](https://en.wikipedia.org/wiki/Methods_of_computing_square_roots)
pub fn sqrt(val: f32) -> f32 {
    let mut val_int: u32 = val.to_bits();

    val_int = val_int.wrapping_sub(1 << 23); /* Subtract 2^m. */
    val_int >>= 1; /* Divide by 2. */
    val_int = val_int.wrapping_add(1 << 29); /* Add ((b + 1) / 2) * 2^m. */

    f32::from_bits(val_int)
}

const fn _next_log2(size: usize, shift: u32) -> u32 {
    if size > (1 << shift) {
        shift + 1
    }
    else if shift == 0 {
        0
    }
    else {
        _next_log2(size, shift - 1)
    }
}

/// Returns the next power of 2
///
/// # Examples
///
/// ```
/// use base::util::math::next_log2;
/// assert_eq!(next_log2(4), 2);
/// assert_eq!(next_log2(5), 3);
/// ```
pub const fn next_log2(size: usize) -> u32 {
    _next_log2(size, (mem::size_of::<usize>() * 8 - 1) as u32)
}

/// Rounds the given value up to the given alignment
///
/// # Examples
///
/// ```
/// use base::util::math::round_up;
/// assert_eq!(round_up(0x123, 0x1000), 0x1000);
/// ```
pub fn round_up<T: PrimInt>(value: T, align: T) -> T {
    (value + align - T::one()) & !(align - T::one())
}

/// Rounds the given value down to the given alignment
///
/// # Examples
///
/// ```
/// use base::util::math::round_dn;
/// assert_eq!(round_dn(0x123, 0x1000), 0x0);
/// ```
pub fn round_dn<T: PrimInt>(value: T, align: T) -> T {
    value & !(align - T::one())
}

/// Returns true if `addr` is aligned to `align`
pub fn is_aligned<T: PrimInt>(addr: T, align: T) -> bool {
    (addr & (align - T::one())) == T::zero()
}

/// Assuming that `startx` < `endx` and `endx` is not included (that means with start=0 and end=10
/// 0 .. 9 is used), the function determines whether the two ranges overlap anywhere.
///
/// Note that both ranges are assumed to be non-empty
pub fn overlaps<T: Ord>(start1: T, end1: T, start2: T, end2: T) -> bool {
    (start1 >= start2 && start1 < end2) // start in range
    || (end1 > start2 && end1 <= end2)  // end in range
    || (start1 < start2 && end1 > end2) // complete overlapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqrt() {
        // note: == comparison with floats does not work in general, but adding an external crate
        // just for this test seems overkill
        assert_eq!(sqrt(4.0), 2.0);
        assert_eq!(sqrt(16.0), 4.0);
    }

    #[test]
    fn test_next_log2() {
        assert_eq!(next_log2(0), 0);
        assert_eq!(next_log2(1), 0);
        assert_eq!(next_log2(2), 1);
        assert_eq!(next_log2(12), 4);
        assert_eq!(next_log2(16), 4);
        assert_eq!(next_log2(63), 6);
        assert_eq!(next_log2(usize::MAX), usize::BITS);
    }

    #[test]
    fn test_round_up() {
        assert_eq!(round_up(10, 4), 12);
        assert_eq!(round_up(10, 16), 16);
        assert_eq!(round_up(0xfff, 0x1000), 0x1000);
        assert_eq!(round_up(0x1000, 0x1000), 0x1000);
        assert_eq!(round_up(0, 0x1000), 0);
        assert_eq!(round_up(1, 0x1000), 0x1000);
    }

    #[test]
    fn test_round_dn() {
        assert_eq!(round_dn(10, 4), 8);
        assert_eq!(round_dn(10, 16), 0);
        assert_eq!(round_dn(0xfff, 0x1000), 0);
        assert_eq!(round_dn(0x1000, 0x1000), 0x1000);
        assert_eq!(round_dn(0, 0x1000), 0);
        assert_eq!(round_dn(1, 0x1000), 0);
    }

    #[test]
    fn test_is_aligned() {
        assert_eq!(is_aligned(0x1000, 0x1000), true);
        assert_eq!(is_aligned(0xfff, 0x1000), false);
        assert_eq!(is_aligned(0x1001, 0x1000), false);
        assert_eq!(is_aligned(4, 4), true);
        assert_eq!(is_aligned(2, 4), false);
    }

    #[test]
    fn test_overlaps() {
        assert_eq!(overlaps(0, 4, 4, 8), false);
        assert_eq!(overlaps(0, 4, 3, 8), true);
        assert_eq!(overlaps(8, 12, 0, 8), false);
        assert_eq!(overlaps(8, 12, 8, 9), true);
        assert_eq!(overlaps(8, 12, 8, 12), true);
        assert_eq!(overlaps(8, 12, 9, 11), true);
        assert_eq!(overlaps(8, 12, 9, 12), true);
        assert_eq!(overlaps(8, 12, 11, 12), true);
        assert_eq!(overlaps(8, 12, 12, 12), false);
        assert_eq!(overlaps(8, 12, 12, 12), false);
    }
}
