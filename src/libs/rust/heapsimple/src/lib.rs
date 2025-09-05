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

use base::cell::StaticCell;
use base::io::LogFlags;
use base::log;
use base::mem;

#[macro_export]
macro_rules! create_heap {
    ($size:expr) => {
        const HEAP_SIZE: usize = $size;

        // the heap area needs to be page-byte aligned
        #[repr(align(4096))]
        struct Heap([u64; HEAP_SIZE / core::mem::size_of::<u64>()]);
        #[used]
        static mut HEAP: Heap = Heap([0; HEAP_SIZE / core::mem::size_of::<u64>()]);

        #[no_mangle]
        extern "C" fn __heap_simple_memory(addr: *mut usize, size: *mut usize) {
            unsafe {
                *addr = &HEAP.0 as *const u64 as usize;
                *size = core::mem::size_of_val(&HEAP.0);
            }
        }
    };
}

extern "C" {
    fn __heap_simple_memory(addr: *mut usize, size: *mut usize);
}

static HEAP_POS: StaticCell<usize> = StaticCell::new(0);
#[global_allocator]
static GLOBAL: MyAllocator = MyAllocator;

struct MyAllocator;

unsafe impl GlobalAlloc for MyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let words = (layout.size() + mem::size_of::<u64>() - 1) / mem::size_of::<u64>();
        let size = words * mem::size_of::<u64>();

        let res = unsafe {
            let mut addr = 0usize;
            let mut size = 0usize;
            __heap_simple_memory(&mut addr as *mut _, &mut size as *mut _);

            let start = addr as *mut u64;
            let end = start.add(size / mem::size_of::<u64>());
            let res = start.add(HEAP_POS.get());
            if res.add(words) > end {
                return core::ptr::null_mut();
            }
            res
        };

        HEAP_POS.set(HEAP_POS.get() + words);
        log!(LogFlags::LibHeap, "heap::alloc({}) -> {:?}", size, res);

        res as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        log!(LogFlags::LibHeap, "heap::free({:?})", ptr);
    }
}
