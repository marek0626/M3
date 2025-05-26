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

use base::cell::{LazyStaticCell, StaticCell};
use base::io::LogFlags;
use base::log;
use base::{libc, mem};

extern "C" {
    fn memcpy(dst: *mut libc::c_void, src: *const libc::c_void, len: usize);
    fn memset(s: *mut libc::c_void, b: u8, len: usize);
}

static HEAP: LazyStaticCell<(usize, usize)> = LazyStaticCell::default();
static HEAP_POS: StaticCell<usize> = StaticCell::new(0);

#[no_mangle]
extern "C" fn __heap_simple_init(addr: usize, size: usize) {
    HEAP.set((addr, size));
}

#[no_mangle]
extern "C" fn __rdl_alloc(size: usize, _align: usize, _err: *mut u8) -> *mut libc::c_void {
    let words = (size + mem::size_of::<u64>() - 1) / mem::size_of::<u64>();
    let size = words * mem::size_of::<u64>();

    let res = unsafe {
        let (addr, size) = HEAP.get();
        let start = addr as *mut u64;
        let end = start.add(size / mem::size_of::<u64>());
        let res = start.add(HEAP_POS.get());
        if res.add(words) > end {
            return core::ptr::null_mut::<libc::c_void>();
        }
        res
    };

    HEAP_POS.set(HEAP_POS.get() + words);
    log!(LogFlags::LibHeap, "heap::alloc({}) -> {:?}", size, res);

    res as *mut libc::c_void
}

#[no_mangle]
extern "C" fn __rdl_dealloc(ptr: *mut libc::c_void, _size: usize, _align: usize) {
    log!(LogFlags::LibHeap, "heap::free({:?}) - ignoring", ptr);
}

#[no_mangle]
extern "C" fn __rdl_realloc(
    ptr: *mut libc::c_void,
    old_size: usize,
    _old_align: usize,
    new_size: usize,
    _new_align: usize,
    _err: *mut u8,
) -> *mut libc::c_void {
    let res = __rdl_alloc(new_size, _new_align, _err);
    unsafe { memcpy(res, ptr, old_size) };

    log!(
        LogFlags::LibHeap,
        "heap::realloc({:?}, {}) -> {:?}",
        ptr,
        new_size,
        res
    );
    res
}

#[no_mangle]
extern "C" fn __rdl_alloc_zeroed(size: usize, _align: usize, _err: *mut u8) -> *mut libc::c_void {
    let res = __rdl_alloc(size, _align, _err);
    unsafe { memset(res, 0, size) };
    log!(LogFlags::LibHeap, "heap::calloc({}) -> {:?}", size, res);
    res
}
