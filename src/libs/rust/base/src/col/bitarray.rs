/*
 * Copyright (C) 2020-2021 Nils Asmussen, Barkhausen Institut
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

use crate::col::Vec;
use crate::mem;
use crate::vec;

/// An array of bits
///
/// Besides storing a sequence of bits, `BitArray` keeps track of the first clear bit in the
/// sequence to provide quick access to this information.
pub struct BitArray {
    bits: usize,
    first_clear: usize,
    words: Vec<usize>,
}

fn word_bits() -> usize {
    mem::size_of::<usize>() * 8
}

fn idx(bit: usize) -> usize {
    bit / word_bits()
}

fn bitpos(bit: usize) -> usize {
    1 << (bit % word_bits())
}

impl BitArray {
    /// Creates a new `BitArray` with the given number of bits
    pub fn new(bits: usize) -> Self {
        let word_count = bits.div_ceil(word_bits());
        BitArray {
            bits,
            words: vec![0; word_count],
            first_clear: 0,
        }
    }

    /// Returns the number of bits in the array
    pub fn size(&self) -> usize {
        self.bits
    }

    /// Returns true if the bit with given index is set
    pub fn is_set(&self, bit: usize) -> bool {
        self.words[idx(bit)] & bitpos(bit) != 0
    }

    /// Returns the index of the first bit that is not set (or self.size() if all are set)
    pub fn first_clear(&self) -> usize {
        self.first_clear
    }

    /// Sets the bit with given index
    pub fn set(&mut self, bit: usize) {
        self.words[idx(bit)] |= bitpos(bit);
        if bit == self.first_clear {
            self.first_clear += 1;
            while self.first_clear < self.bits && self.is_set(self.first_clear) {
                self.first_clear += 1;
            }
        }
    }

    /// Clears the bit with given index
    pub fn clear(&mut self, bit: usize) {
        self.words[idx(bit)] &= !bitpos(bit);
        if bit < self.first_clear {
            self.first_clear = bit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut ba = BitArray::new(4);
        assert_eq!(ba.size(), 4);
        assert_eq!(ba.first_clear(), 0);
        assert_eq!(ba.is_set(0), false);

        ba.set(2);
        assert_eq!(ba.is_set(2), true);

        ba.set(3);
        assert_eq!(ba.is_set(3), true);

        ba.clear(3);
        assert_eq!(ba.is_set(3), false);
    }

    #[test]
    fn large() {
        let mut ba = BitArray::new(1024);
        assert_eq!(ba.size(), 1024);

        ba.set(0);
        assert_eq!(ba.first_clear(), 1);

        ba.set(1);
        assert_eq!(ba.first_clear(), 2);

        ba.set(500);
        assert_eq!(ba.first_clear(), 2);
        assert_eq!(ba.is_set(500), true);

        ba.set(1023);
        assert_eq!(ba.first_clear(), 2);
        assert_eq!(ba.is_set(1023), true);

        ba.clear(1);
        assert_eq!(ba.first_clear(), 1);

        ba.clear(0);
        assert_eq!(ba.first_clear(), 0);
    }

    #[test]
    fn unaligned() {
        let mut ba = BitArray::new(65);
        ba.set(64);
        assert_eq!(ba.is_set(64), true);
    }

    #[test]
    fn first_clear() {
        let mut ba = BitArray::new(4);
        assert_eq!(ba.size(), 4);
        assert_eq!(ba.first_clear(), 0);
        assert_eq!(ba.is_set(0), false);

        ba.set(0);
        assert_eq!(ba.first_clear(), 1);
        ba.set(1);
        assert_eq!(ba.first_clear(), 2);
        ba.set(2);
        assert_eq!(ba.first_clear(), 3);
        ba.set(3);
        assert_eq!(ba.first_clear(), 4);

        ba.clear(3);
        assert_eq!(ba.first_clear(), 3);
        ba.clear(0);
        assert_eq!(ba.first_clear(), 0);
        ba.clear(1);
        assert_eq!(ba.first_clear(), 0);
        ba.clear(2);
        assert_eq!(ba.first_clear(), 0);

        ba.set(1);
        ba.set(2);
        ba.set(3);
        assert_eq!(ba.first_clear(), 0);
        ba.set(0);
        assert_eq!(ba.first_clear(), 4);

        ba.clear(0);
        ba.clear(1);
        ba.clear(2);
        ba.clear(3);

        ba.set(1);
        ba.set(2);
        assert_eq!(ba.first_clear(), 0);
        ba.set(0);
        assert_eq!(ba.first_clear(), 3);
    }
}
