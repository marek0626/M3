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

use core::fmt;
use core::ops;

use num_traits::PrimInt;

use crate::col::LinkedList;
use crate::errors::{Code, Error};
use crate::util::math;

struct Area<T: PrimInt> {
    addr: T,
    size: T,
}

impl<T: PrimInt> Area<T> {
    pub fn new(addr: T, size: T) -> Self {
        Area { addr, size }
    }
}

impl<T: PrimInt + fmt::LowerHex> fmt::Debug for Area<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Area[addr={:#x}, size={:#x}]", self.addr, self.size)
    }
}

/// The memory map, allowing allocs and frees of memory areas
pub struct MemMap<T: PrimInt> {
    areas: LinkedList<Area<T>>,
}

impl<T: PrimInt + ops::AddAssign + ops::SubAssign> MemMap<T> {
    /// Creates a new memory map from `addr` to `addr`+`size`.
    pub fn new(addr: T, size: T) -> Self {
        let mut areas = LinkedList::new();
        areas.push_back(Area::new(addr, size));
        MemMap { areas }
    }

    /// Allocates a region of `size` bytes, aligned by `align`.
    pub fn allocate(&mut self, size: T, align: T) -> Result<T, Error> {
        // find an area with sufficient space
        let mut cur = self.areas.cursor_front_mut();
        let a: Option<&mut Area<T>> = loop {
            match cur.current() {
                None => break None,
                Some(a) => {
                    let diff = math::round_up(a.addr, align) - a.addr;
                    if a.size > diff && a.size - diff >= size {
                        break Some(a);
                    }
                },
            }
            cur.move_next();
        };

        match a {
            None => Err(Error::new(Code::OutOfMem)),
            Some(a) => {
                // if we need to do some alignment, create a new area in front of a
                let org_addr = a.addr;
                let diff = math::round_up(a.addr, align) - org_addr;
                if diff != T::zero() {
                    a.addr += diff;
                    a.size -= diff;
                }

                // take it from the front
                let res = a.addr;
                a.size -= size;
                a.addr += size;

                // if the area is empty now, remove it
                if a.size == T::zero() {
                    cur.remove_current();
                }
                if diff != T::zero() {
                    cur.insert_before(Area::new(org_addr, diff));
                }

                Ok(res)
            },
        }
    }

    /// Free's the given memory region defined by `addr` and `size`.
    pub fn free(&mut self, addr: T, size: T) {
        // find the area behind ours
        let mut cur = self.areas.cursor_front_mut();
        loop {
            match cur.current() {
                None => break,
                Some(n) => {
                    if addr <= n.addr {
                        break;
                    }
                },
            }
            cur.move_next();
        }

        let res = {
            let cur_rdonly = cur.as_cursor();
            let n = cur_rdonly.current();
            let p = cur_rdonly.peek_prev();
            match (p, n) {
                // merge with prev and next
                (Some(ref mut p), Some(n)) if p.addr + p.size == addr && addr + size == n.addr => {
                    let nsize = n.size;
                    let p = cur.peek_prev().unwrap();
                    p.size += size + nsize;
                    1
                },

                // merge with prev
                (Some(ref mut p), _) if p.addr + p.size == addr => {
                    let p = cur.peek_prev().unwrap();
                    p.size += size;
                    0
                },

                // merge with next
                (_, Some(ref mut n)) if addr + size == n.addr => {
                    let n = cur.current().unwrap();
                    n.addr -= size;
                    n.size += size;
                    0
                },

                (_, _) => 2,
            }
        };

        if res == 1 {
            cur.remove_current();
        }
        else if res == 2 {
            cur.insert_before(Area::new(addr, size));
        }
    }

    /// Returns the size of the largest contiguous free space
    pub fn largest_contiguous(&self) -> Option<T> {
        self.areas
            .iter()
            .max_by(|a, b| a.size.cmp(&b.size))
            .map(|a| a.size)
    }

    /// Returns a pair of the remaining space and the number of areas.
    pub fn size(&self) -> (T, usize) {
        let mut total = T::zero();
        for a in self.areas.iter() {
            total += a.size;
        }
        (total, self.areas.len())
    }
}

impl<T: PrimInt + fmt::LowerHex> fmt::Debug for MemMap<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "[")?;
        for a in &self.areas {
            writeln!(f, "    {:?}", a)?;
        }
        write!(f, "  ]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut m = MemMap::new(0, 0x1000);

        assert_eq!(m.allocate(0x100, 0x10), Ok(0x0));
        assert_eq!(m.allocate(0x100, 0x10), Ok(0x100));
        assert_eq!(m.allocate(0x100, 0x10), Ok(0x200));

        m.free(0x100, 0x100);
        m.free(0x0, 0x100);

        assert_eq!(
            m.allocate(0x1000, 0x10).map_err(|e| e.code()),
            Err(Code::OutOfMem)
        );
        assert_eq!(m.allocate(0x200, 0x10), Ok(0x0));

        m.free(0x200, 0x100);
        m.free(0x0, 0x200);

        assert_eq!(m.size(), (0x1000, 1));
    }

    #[test]
    fn largest_contiguous() {
        let mut m = MemMap::new(0, 1000);

        assert_eq!(m.largest_contiguous(), Some(1000));

        assert_eq!(m.allocate(200, 1), Ok(0));
        assert_eq!(m.allocate(200, 1), Ok(200));
        assert_eq!(m.allocate(200, 1), Ok(400));
        assert_eq!(m.allocate(200, 1), Ok(600));

        assert_eq!(m.largest_contiguous(), Some(200));

        m.free(400, 200);
        assert_eq!(m.largest_contiguous(), Some(200));

        m.free(200, 200);
        assert_eq!(m.largest_contiguous(), Some(400));
    }

    #[test]
    fn alloc() {
        let mut m = MemMap::new(0, 0x1000);

        assert_eq!(m.allocate(0x50, 0x100), Ok(0));
        assert_eq!(m.allocate(0x50, 0x100), Ok(0x100));
        assert_eq!(m.allocate(0x50, 0x100), Ok(0x200));

        // each allocation leaves a 0x50 hole after it (@ 0x50, @ 0x150, @ 0x250). the last one
        // goes to the end of the area, making it the largest contiguous region.
        assert_eq!(m.largest_contiguous(), Some(0x1000 - 0x250));
    }

    #[test]
    fn free() {
        let mut m = MemMap::new(0, 1200);

        assert_eq!(m.allocate(200, 1), Ok(0));
        assert_eq!(m.allocate(200, 1), Ok(200));
        assert_eq!(m.allocate(200, 1), Ok(400));
        assert_eq!(m.allocate(200, 1), Ok(600));
        assert_eq!(m.allocate(200, 1), Ok(800));
        assert_eq!(m.allocate(200, 1), Ok(1000));

        m.free(800, 200);
        m.free(400, 200);
        // merge with prev and next
        m.free(600, 200);

        // merge with prev
        m.free(1000, 200);
    }
}
