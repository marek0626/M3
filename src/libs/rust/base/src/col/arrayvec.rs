/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2020 Nils Asmussen, Barkhausen Institut
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

//! Very simple array-based vector

use core::fmt;
use core::mem::MaybeUninit;
use core::ops::Deref;

/// Very simple array-based vector with a fixed capacity
pub struct ArrayVec<T, const CAP: usize> {
    elements: [MaybeUninit<T>; CAP],
    len: usize,
}

impl<T, const CAP: usize> Default for ArrayVec<T, CAP> {
    fn default() -> Self {
        Self {
            elements: [const { MaybeUninit::uninit() }; CAP],
            len: Default::default(),
        }
    }
}

impl<T, const CAP: usize> ArrayVec<T, CAP> {
    /// Push a new element to the end
    ///
    /// # Panics
    ///
    /// Panics if already full.
    pub fn push(&mut self, value: T) {
        let slot = self
            .elements
            .get_mut(self.len)
            .expect("cannot insert into full ArrayVec");
        *slot = MaybeUninit::new(value);
        // Cannot overflow as it can reach at most CAP.
        self.len += 1;
    }

    /// Pop an element from the back
    ///
    /// # Panics
    ///
    /// Panics if empty.
    pub fn pop(&mut self) -> T {
        self.len = self
            .len
            .checked_sub(1)
            .expect("cannot pop from empty ArrayVec");

        // SAFETY: The index is valid because the length value did not just
        // underflow and an overflow is prevented in the push method.
        let slot = unsafe { self.elements.get_unchecked_mut(self.len) };
        let value = core::mem::replace(slot, MaybeUninit::uninit());

        // SAFETY: There is an initialized element in the slot because the push
        // method only inserts initialized data.
        unsafe { value.assume_init() }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T, const CAP: usize> Deref for ArrayVec<T, CAP> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: The elements in the range 0..self.len are always in range.
        let slice = unsafe { self.elements.get_unchecked(0..self.len) };
        // SAFETY: All elements in the slice are initialized.
        unsafe { core::mem::transmute::<&[MaybeUninit<T>], &[T]>(slice) }
    }
}

impl<T, const CAP: usize> fmt::Debug for ArrayVec<T, CAP>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        let values: &[u8] = &[4, 5, 3, 8, 6, 5];
        let mut vec = ArrayVec::<u8, 10>::default();

        assert!(vec.is_empty());
        assert_eq!(&*vec, &[]);

        for &value in values {
            vec.push(value);
            assert!(!vec.is_empty());
        }

        assert_eq!(&*vec, values);

        for &value in values.iter().rev() {
            assert!(!vec.is_empty());
            assert_eq!(vec.pop(), value);
        }

        assert!(vec.is_empty());
        assert_eq!(&*vec, &[]);
    }
}
