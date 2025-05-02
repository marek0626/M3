/*
 * Copyright (C) 2020 Nils Asmussen, Barkhausen Institut
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

#pragma once

#include <base/stream/Format.h>
#include <base/util/Math.h>

#include <assert.h>
#include <utility>

namespace m3 {

struct InPlaceArea {
    size_t size;
    InPlaceArea *next;
};

/**
 * Manages memory areas by storing the meta data in-place.
 *
 * The InPlaceAreaManager manages a contiguous piece of memory by putting meta data inside this
 * piece of memory. This is in contrast to the AreaManager, which uses heap allocations for meta
 * data.
 *
 * Note that the implementation does not align the allocated areas, which means that the alignment
 * depends on the current state of the manager. Furthermore, each allocated area needs to be a
 * multiple of InPlaceArea.
 */
class InPlaceAreaManager {
public:
    /**
     * Creates an new in-place area manager with given region.
     *
     * @param addr the base address of the memory region
     * @param size the size of the region
     */
    explicit InPlaceAreaManager(void *addr = nullptr, size_t size = 0) : list(), end() {
        if(addr)
            set_region(addr, size);
    }

    InPlaceAreaManager(const InPlaceAreaManager &) = delete;
    InPlaceAreaManager &operator=(const InPlaceAreaManager &) = delete;

    /**
     * Sets the managed region to the given one.
     *
     * This requires that the region has not been set yet.
     *
     * @param addr the base address of the memory region
     * @param size the size of the region
     */
    void set_region(void *addr, size_t size) {
        assert(list == nullptr);
        list = reinterpret_cast<InPlaceArea *>(addr);
        list->size = size;
        list->next = nullptr;
        end = reinterpret_cast<uintptr_t>(addr) + size;
    }

    /**
     * Appends the given amount of space to the last area
     *
     * @param size the amount of space
     * @return true on success
     */
    bool append(size_t size) {
        InPlaceArea *a;
        for(a = list; a != nullptr; a = a->next) {
            if(reinterpret_cast<uintptr_t>(a) + a->size == end)
                break;
        }
        if(a == nullptr)
            return false;
        a->size += size;
        end += size;
        return true;
    }

    /**
     * Allocates an area of given size.
     *
     * Note that the size needs to be a multiple of sizeof(InPlaceArea).
     *
     * @param size the size of the area in bytes
     * @return the address, if space was found, nullptr otherwise
     */
    void *allocate(size_t size) {
        assert((size & (sizeof(InPlaceArea) - 1)) == 0);

        InPlaceArea *a;
        InPlaceArea *p = nullptr;
        for(a = list; a != nullptr; p = a, a = a->next) {
            if(a->size >= size)
                break;
        }
        if(a == nullptr)
            return nullptr;

        // take it from the front
        void *res = a;
        InPlaceArea *n = a->next;
        // if there is space left, create a new area
        if(a->size > size) {
            n = reinterpret_cast<InPlaceArea *>(reinterpret_cast<uintptr_t>(a) + size);
            n->size = a->size - size;
            n->next = a->next;
        }
        // in any case, make the prev point to the new next
        if(p)
            p->next = n;
        else
            list = n;
        return res;
    }

    /**
     * Frees the area at <ptr> with <size> bytes.
     *
     * @param ptr the address of the area
     * @param size the size of the area
     */
    void free(void *ptr, size_t size) {
        uintptr_t addr = reinterpret_cast<uintptr_t>(ptr);

        // find the area behind ours
        InPlaceArea *n, *p = nullptr;
        for(n = list; n != nullptr && addr > reinterpret_cast<uintptr_t>(n); p = n, n = n->next)
            ;

        // merge with prev and next
        InPlaceArea *nn = nullptr;
        if(p && reinterpret_cast<uintptr_t>(p) + p->size == addr && n &&
           addr + size == reinterpret_cast<uintptr_t>(n)) {
            p->size += size + n->size;
            p->next = n->next;
        }
        // merge with prev
        else if(p && reinterpret_cast<uintptr_t>(p) + p->size == addr) {
            p->size += size;
        }
        // merge with next
        else if(n && addr + size == reinterpret_cast<uintptr_t>(n)) {
            nn = reinterpret_cast<InPlaceArea *>(addr);
            nn->size = n->size + size;
            nn->next = n->next;
        }
        // create new area between them
        else {
            nn = reinterpret_cast<InPlaceArea *>(addr);
            nn->size = size;
            nn->next = n;
        }

        // adjust prev for new area
        if(nn) {
            if(p)
                p->next = nn;
            else
                list = nn;
        }
    }

    /**
     * Just for debugging/testing: Determines the total number of free bytes in the map
     *
     * @return a pair of the free bytes and the number of areas
     */
    std::pair<size_t, size_t> size() const {
        size_t total = 0;
        size_t areas = 0;
        for(auto *a = list; a != nullptr; a = a->next) {
            total += a->size;
            areas++;
        }
        return std::make_pair(total, areas);
    }

    void format(OStream &os, const FormatSpecs &) const {
        size_t total = size().first;
        format_to(os, "Total: {} KiB:\n"_cf, total / 1024);
        for(auto *a = list; a != nullptr; a = a->next)
            format_to(os, "\t@ {:p}, {} KiB\n"_cf, reinterpret_cast<uintptr_t>(a), a->size / 1024);
    }

private:
    InPlaceArea *list;
    uintptr_t end;
};

}
