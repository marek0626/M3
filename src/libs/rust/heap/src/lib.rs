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

#![no_std]

use core::alloc::{GlobalAlloc, Layout};

use base::io::LogFlags;
use base::libc;
use base::log;

extern "C" {
    /// Allocates `size` bytes on the heap
    fn malloc(size: usize) -> *mut libc::c_void;

    /// Frees the area at `p`
    fn free(p: *mut libc::c_void);
}

struct MyAllocator;

unsafe impl GlobalAlloc for MyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let res = unsafe { malloc(layout.size()) as *mut u8 };
        log!(
            LogFlags::LibHeap,
            "heap::alloc({}) -> {:?}",
            layout.size(),
            res
        );
        res
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        log!(LogFlags::LibHeap, "heap::free({:?})", ptr);
        unsafe { free(ptr as *mut libc::c_void) };
    }
}

#[global_allocator]
static GLOBAL: MyAllocator = MyAllocator;
