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

//! This is the global Rust allocator of the kernel
//!
//! It uses slab-based allocations for small allocations and falls back to _malloc_ otherwise.

use core::{
    alloc::{GlobalAlloc, Layout},
    mem::size_of,
    ptr::{null_mut, NonNull},
};

use base::{
    cell::StaticRefCell,
    io::LogFlags,
    libc::{self, MAX_ALIGN},
    log,
};

/// An (empty) area on one of the slabs
#[repr(C)]
struct Area {
    /// Next in the free list
    next: Option<NonNull<Area>>,
}

/// A slab allocator for a specific size
///
/// When full, the allocator requests new memory via _malloc_ with a size large enough to cover
/// multiple slab areas to reduce the average allocation latency.
/// Free areas are managed as a free list.
struct Slab {
    /// Head of the free list
    free: Option<NonNull<Area>>,
    /// Size of areas used by this slab allocator
    ///
    /// A size of [`None`] indicates the fallback-mode of the slab allocator which always uses
    /// malloc and does not use slabs.
    size: Option<usize>,
}

impl Slab {
    /// Number of areas that should fit into a newly allocated slab of areas.
    const NEW_AREA_COUNT: usize = 64;

    const fn new(size: Option<usize>) -> Self {
        if let Some(size) = size {
            // Assert that we always align to max_align_t.
            assert!(size >= MAX_ALIGN);
            // Assert that the Area can align nicely into the beginning of each size-sized chunk.
            let layout = Layout::new::<Area>().pad_to_align();
            assert!(size >= layout.size());
            assert!(size % layout.size() == 0);
        }
        Self { free: None, size }
    }

    /// Extend the memory of the allocator by allocating a new slab.
    ///
    /// Returns `false` on failure.
    ///
    /// # Panics
    ///
    /// Panics if `self.size` is [`None`].
    #[inline(never)]
    #[must_use]
    unsafe fn extend(&mut self) -> bool {
        let Some(size) = self.size
        else {
            panic!("extend called on unsized slab");
        };

        // SAFETY: Malloc always returns memory aligned to the word size, which is enough for Area
        // and the allocated objects.
        let Some(mut a) = NonNull::new(malloc(size * Self::NEW_AREA_COUNT).cast::<Area>())
        else {
            return false;
        };

        // Add all areas to free list.
        for _ in 0..Self::NEW_AREA_COUNT {
            a.write(Area { next: self.free });
            self.free = Some(a);
            // SAFETY: The new area is properly aligned because of the checks in new().
            a = a.byte_add(size);
        }

        true
    }

    /// Create an allocator for the given layout
    ///
    /// Returns `null` if the `layout` does not fit the allocator or the allocation failed.
    /// If `zeroed` is true, the memory will be initialized to zero.
    /// This uses `calloc` inside the fallback allocator.
    ///
    /// # Safety
    ///
    /// See [`GlobalAlloc::alloc`].
    unsafe fn alloc(&mut self, layout: Layout, zeroed: bool) -> *mut u8 {
        if !self.fits(layout) {
            return null_mut();
        }

        match self.size {
            Some(_) => {
                // Extend slab if needed.
                if self.free.is_none() && !self.extend() {
                    return null_mut();
                }

                let res = self.free.expect("slab not extended");
                self.free = (*res.as_ptr()).next;
                let ptr = res.as_ptr().cast();
                if zeroed {
                    core::ptr::write_bytes(ptr, 0, layout.size());
                }
                ptr
            },

            None => {
                let alloc_size = Self::ceil_size(layout.size());
                if zeroed {
                    calloc(1, alloc_size)
                }
                else {
                    malloc(alloc_size)
                }
                .cast()
            },
        }
    }

    /// Free memory
    ///
    /// Returns `false` if the `layout` does not fit the allocator.
    ///
    /// # Safety
    ///
    /// Undefined behavior if the allocation fits but still was not allocated via `self`.
    #[must_use]
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) -> bool {
        if !self.fits(layout) {
            return false;
        }

        match self.size {
            Some(_) => {
                // SAFETY: ptr is guaranteed to be non-null because it must have been successfully
                // allocated by alloc().
                let area = ptr.cast::<Area>();
                area.write(Area { next: self.free });
                self.free = NonNull::new(area);
            },

            None => free(ptr.cast()),
        }
        true
    }

    /// Would an allocation with this `layout` fit into this allocator?
    fn fits(&self, layout: Layout) -> bool {
        let by_size = match self.size {
            Some(self_size) => layout.size() <= self_size,
            None => true,
        };
        // We only guarantee alignment to word size.
        let by_align = layout.align() <= libc::MAX_ALIGN;
        by_size && by_align
    }

    /// Ensures that the `size` is large enough to fit an `Area`
    fn ceil_size(size: usize) -> usize {
        core::cmp::max(size, size_of::<Area>())
    }

    /// Reallocate to `new_size` inside this allocator
    ///
    /// # Safety
    ///
    /// The `ptr` must originate from this allocator and the `new_size` must fit this allocator.
    unsafe fn realloc(&self, ptr: *mut u8, new_size: usize) -> *mut u8 {
        match self.size {
            Some(_) => {
                // SAFETY: The caller guarantees that the new_size fits.
                ptr
            },
            None => realloc(ptr.cast(), new_size).cast(),
        }
    }
}

/// An allocator combining multiple fixed-sized slab allocators
///
/// It chooses the closest-fit allocator from a list of slab allocators.
struct SlabAllocator {
    /// The individual slab allocators
    ///
    /// A [`StaticRefCell`] is used to allow allocation using `&self` to satisfy [`GlobalAlloc`].
    slabs: StaticRefCell<[Slab; Self::SLABS.len()]>,
}

impl SlabAllocator {
    /// Available slabs
    ///
    /// The slab sizes should be strictly increasing with the last being unsized.
    const SLABS: [Slab; 3] = [Slab::new(Some(64)), Slab::new(Some(128)), Slab::new(None)];

    const fn new() -> Self {
        let slabs = StaticRefCell::new(Self::SLABS);
        Self { slabs }
    }

    /// Return the index of the slab allocator that will be used for an allocation of this size
    const fn get_slab(obj_size: usize) -> usize {
        let mut i = 0;
        while i < Self::SLABS.len() {
            let size = Self::SLABS[i].size;
            if size.is_none() || size.unwrap() >= obj_size {
                return i;
            }
            i += 1;
        }
        panic!("found no slab fitting this size")
    }

    /// Allocate memory with configurable zeroing
    unsafe fn real_alloc(&self, layout: Layout, zeroed: bool) -> *mut u8 {
        let mut slabs = self.slabs.borrow_mut();
        // It is convention that the last slab fits all sizes.
        let (size, res) = slabs
            .iter_mut()
            .map(|slab| (slab.size, unsafe { slab.alloc(layout, zeroed) }))
            .find(|(_, res)| !res.is_null())
            .unwrap_or((None, null_mut()));

        log!(
            LogFlags::KernSlab,
            "alloc(sz={}, s={:?}, z={}) -> {:#x}",
            layout.size(),
            size,
            zeroed,
            res as usize
        );
        res
    }

    /// Reallocate memory to a new size
    ///
    /// If the new size would use the same allocator, reallocate inside this allocator.
    /// Else, reallocate inside the destined allocator.
    ///
    /// # Safety
    ///
    /// See [`GlobalAlloc::realloc`].
    unsafe fn real_realloc(&self, new_size: usize, layout: Layout, ptr: *mut u8) -> *mut u8 {
        // SAFETY: The caller of `GlobalAlloc::realloc` ensures that the `new_size` does not
        // overflow. `layout.align()` comes from a `Layout` and is thus guaranteed to be valid.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };

        let mut new_ptr: *mut u8 = null_mut();
        let mut slabs = self.slabs.borrow_mut();
        for slab in slabs.iter_mut() {
            let old_fits = slab.fits(layout);
            let new_fits = slab.fits(new_layout);
            if old_fits != new_fits {
                break;
            }
            if old_fits && new_fits {
                new_ptr = slab.realloc(ptr, new_size);
                break;
            }
        }
        if !new_ptr.is_null() {
            return new_ptr;
        }
        drop(slabs);

        // SAFETY: The caller of `GlobalAlloc::realloc` ensures that `new_layout` is greater than
        // zero.
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            // SAFETY: The previously allocated block cannot overlap the newly allocated block.
            // The safety contract for `dealloc` must be upheld by the caller of
            // `GlobalAlloc::realloc`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    ptr,
                    new_ptr,
                    core::cmp::min(layout.size(), new_size),
                );
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}

unsafe impl GlobalAlloc for SlabAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.real_alloc(layout, false)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut slabs = self.slabs.borrow_mut();
        // The first fitting slab allocator should be the one that was used for allocation.
        let (size, _) = slabs
            .iter_mut()
            .map(|slab| (slab.size, unsafe { slab.dealloc(ptr, layout) }))
            .find(|(_, res)| *res)
            .expect("no fitting allocator found during deallocation");

        log!(
            LogFlags::KernSlab,
            "free(p={:#x}, sz={}, s={:?})",
            ptr as usize,
            layout.size(),
            size
        );
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.real_alloc(layout, true)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let res = self.real_realloc(new_size, layout, ptr);
        log!(
            LogFlags::KernSlab,
            "realloc(p={:#x}, oldsz={}, newsz={}) -> {:#x}",
            ptr as usize,
            layout.size(),
            new_size,
            res as usize
        );
        res
    }
}

/// This variable is set as the global allocator
#[global_allocator]
static ALLOCATOR: SlabAllocator = SlabAllocator::new();

/// Check that an allocation of `obj_size` would use the slab allocator at index `slab`.
#[allow(dead_code)]
pub const fn fits_slab(obj_size: usize, slab: usize) -> bool {
    let i = SlabAllocator::get_slab(obj_size);
    i == slab
}

/// Return the area size used by the slab allocator that would be used for allocations of this size
///
/// Returns an estimated memory usage for allocations that would use the fallback allocator.
pub const fn area_size(obj_size: usize) -> usize {
    let i = SlabAllocator::get_slab(obj_size);
    match SlabAllocator::SLABS[i].size {
        Some(size) => size,
        None => {
            // since we are using musl's heap, it's hard to say what the overhead per allocation is.
            // that depends on whether we needed a new "group" or not, for example. as an estimate
            // use 64 bytes.
            obj_size + 64
        },
    }
}

extern "C" {
    fn malloc(size: usize) -> *mut libc::c_void;
    fn calloc(n: usize, size: usize) -> *mut libc::c_void;
    fn realloc(p: *mut libc::c_void, size: usize) -> *mut libc::c_void;
    fn free(p: *mut libc::c_void);
}
