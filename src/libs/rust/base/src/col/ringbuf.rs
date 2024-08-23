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
            else if self.rd_pos > req_size && size > 0 {
                self.last = self.wr_pos;
                self.wr_pos = size;
            }
        }
        else if self.rd_pos - self.wr_pos >= req_size {
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
    }
}
