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

use core::cmp;

/// A ringbuffer with variably-sized items
#[derive(Debug)]
pub struct VarRingBuf {
    size: usize,
    cap: usize,
    rd_pos: usize,
    wr_pos: usize,
    last: usize,
}

impl VarRingBuf {
    /// Creates a new ringbuffer with `cap` bytes capacity.
    pub fn new(cap: usize) -> Self {
        VarRingBuf {
            size: 0,
            cap,
            rd_pos: 0,
            wr_pos: 0,
            last: cap,
        }
    }

    /// Returns true if the ringbuffer is empty, i.e., no items can be read
    pub fn empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the number of bytes in the ringbuffer
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the ringbuffer's capacity in bytes
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Determines the write position for inserting `size` bytes.
    ///
    /// Note that `size` has to be greater than 0.
    pub fn get_write_pos(&self, size: usize) -> Option<usize> {
        if self.cap - self.size < size {
            return None;
        }

        if self.wr_pos >= self.rd_pos {
            if self.cap - self.wr_pos >= size {
                return Some(self.wr_pos);
            }
            else if self.rd_pos >= size {
                return Some(0);
            }
        }
        else if self.rd_pos - self.wr_pos >= size {
            return Some(self.wr_pos);
        }
        None
    }

    /// Determines the next read position and the amount of bytes available to read. If there is
    /// something to read, the function returns a tuple with the position and the amount. Otherwise,
    /// it returns None.
    ///
    /// Note that `size` has to be greater than 0.
    pub fn get_read_pos(&self, size: usize) -> Option<(usize, usize)> {
        if self.empty() {
            return None;
        }

        let rpos = if self.rd_pos == self.last {
            0
        }
        else {
            self.rd_pos
        };

        if self.wr_pos > rpos {
            Some((rpos, cmp::min(self.wr_pos - rpos, size)))
        }
        else {
            Some((rpos, cmp::min(cmp::min(self.cap, self.last) - rpos, size)))
        }
    }

    /// Advances the write position by `size`.
    ///
    /// The argument `req_size` specifies the number of bytes that have been passed to
    /// get_write_pos. It is used to detect a potential wrap around to zero by get_write_pos, even
    /// if `size` would not require one.
    ///
    /// Note that `req_size` and `size` have to be greater than 0.
    pub fn push(&mut self, req_size: usize, size: usize) {
        if self.wr_pos >= self.rd_pos {
            if self.cap - self.wr_pos >= req_size {
                self.wr_pos += size;
            }
            else {
                self.last = self.wr_pos;
                self.wr_pos = size;
            }
        }
        else {
            self.wr_pos += size;
        }
        self.size += size;
    }

    /// Advances the read position by `size`.
    ///
    /// Note that `size` has to be greater than 0.
    pub fn pull(&mut self, size: usize) {
        assert!(!self.empty());
        if self.rd_pos == self.last {
            self.rd_pos = 0;
            self.last = self.cap;
        }
        self.rd_pos += size;
        self.size -= size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut rb = VarRingBuf::new(10);
        assert_eq!(rb.capacity(), 10);
        assert!(rb.empty());
        rb.get_write_pos(1).unwrap();
        rb.push(1, 1);
        assert!(!rb.empty());
        assert_eq!(rb.size(), 1);
        assert_eq!(rb.get_write_pos(10), None);

        let (_rpos, rlen) = rb.get_read_pos(4).unwrap();
        assert_eq!(rlen, 1);
        rb.pull(rlen);

        assert!(rb.empty());
        rb.get_write_pos(10);
    }

    #[test]
    fn full() {
        let mut rb = VarRingBuf::new(10);

        let wpos1 = rb.get_write_pos(5).unwrap();
        assert_eq!(wpos1, 0);
        rb.push(5, 5);

        let wpos2 = rb.get_write_pos(5).unwrap();
        assert_eq!(wpos2, 5);
        rb.push(5, 5);

        let rpos1 = rb.get_read_pos(5).unwrap().0;
        assert_eq!(rpos1, 0);
        rb.pull(5);

        let wpos3 = rb.get_write_pos(5).unwrap();
        assert_eq!(wpos3, 0);
        rb.push(5, 5);

        assert_eq!(rb.get_write_pos(5), None);

        assert_eq!(rb.rd_pos, 5);
        assert_eq!(rb.wr_pos, 5);
        assert_eq!(rb.size, 10);
        assert_eq!(rb.cap, 10);
    }

    #[test]
    fn limits() {
        let mut rb = VarRingBuf::new(10);
        assert_eq!(rb.capacity(), 10);
        assert!(rb.empty());
        assert_eq!(rb.get_read_pos(1), None);

        let wpos = rb.get_write_pos(10).unwrap();
        assert_eq!(wpos, 0);
        rb.push(10, 10);
        assert!(!rb.empty());
        assert_eq!(rb.size(), 10);

        let (rpos, rlen) = rb.get_read_pos(10).unwrap();
        assert_eq!(rpos, 0);
        assert_eq!(rlen, 10);
        rb.pull(rlen);

        assert!(rb.empty());
    }

    #[test]
    fn small_steps() {
        let mut rb = VarRingBuf::new(4);

        rb.get_write_pos(1).unwrap();
        rb.push(1, 1);
        rb.get_write_pos(1).unwrap();
        rb.push(1, 1);
        rb.get_write_pos(1).unwrap();
        rb.push(1, 1);
        rb.get_write_pos(1).unwrap();
        rb.push(1, 1);
        assert_eq!(rb.get_write_pos(1), None);

        rb.get_read_pos(1).unwrap();
        rb.pull(1);
        rb.get_read_pos(1).unwrap();
        rb.pull(1);
        rb.get_read_pos(1).unwrap();
        rb.pull(1);
        rb.get_read_pos(1).unwrap();
        rb.pull(1);
        assert!(rb.empty());
    }

    #[test]
    fn wrap_around() {
        let mut rb = VarRingBuf::new(4);
        assert_eq!(rb.get_write_pos(2).unwrap(), 0);
        rb.push(2, 2);
        assert_eq!(rb.get_read_pos(2).unwrap(), (0, 2));
        rb.pull(2);
        assert_eq!(rb.get_write_pos(2).unwrap(), 2);
        rb.push(2, 2);
        assert_eq!(rb.get_read_pos(2).unwrap(), (2, 2));
        rb.pull(2);
        assert_eq!(rb.get_write_pos(2).unwrap(), 0);
        rb.push(2, 2);
        assert_eq!(rb.get_read_pos(2).unwrap(), (0, 2));
        rb.pull(2);
        assert!(rb.empty());
    }

    #[test]
    fn diff_steps() {
        let mut rb = VarRingBuf::new(4);
        assert_eq!(rb.get_write_pos(4).unwrap(), 0);
        rb.push(4, 4);
        assert_eq!(rb.get_read_pos(2).unwrap(), (0, 2));
        rb.pull(2);
        assert_eq!(rb.get_read_pos(2).unwrap(), (2, 2));
        rb.pull(2);
        assert!(rb.empty());
        assert_eq!(rb.get_write_pos(4).unwrap(), 0);
        rb.push(4, 4);
        assert_eq!(rb.get_read_pos(3).unwrap(), (0, 3));
        rb.pull(3);
        assert_eq!(rb.get_write_pos(1).unwrap(), 0);
        rb.push(1, 1);
        assert_eq!(rb.get_read_pos(4).unwrap(), (3, 1));
        rb.pull(1);
        assert_eq!(rb.get_read_pos(2).unwrap(), (0, 1));
        rb.pull(1);
        assert!(rb.empty());
    }

    #[test]
    fn read_behind() {
        let mut rb = VarRingBuf::new(6);
        assert_eq!(rb.get_write_pos(6).unwrap(), 0);
        rb.push(6, 6);
        assert_eq!(rb.get_read_pos(4).unwrap(), (0, 4));
        rb.pull(4);
        assert_eq!(rb.get_write_pos(2).unwrap(), 0);
        rb.push(2, 2);
        assert_eq!(rb.get_write_pos(2).unwrap(), 2);
        rb.push(2, 2);
    }
}
