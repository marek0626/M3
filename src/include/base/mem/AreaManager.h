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

#include <memory>
#include <utility>

namespace m3 {

struct Area {
    goff_t addr;
    size_t size;
    Area *next;
};

/**
 * Manages memory areas by storing the meta data via heap allocations.
 *
 * The AreaManager manages a contiguous piece of memory by using heap allocations to store meta
 * data. This is in contrast to the InPlaceAreaManager, which stores this meta data in-place.
 */
class AreaManager {
public:
    /**
     * Creates a memory-map for given area
     *
     * @param area the area of address and size
     */
    explicit AreaManager(std::pair<goff_t, size_t> area) : AreaManager(area.first, area.second) {
    }

    /**
     * Creates a memory-map of <size> bytes.
     *
     * @param addr the base address
     * @param size the mem size
     */
    explicit AreaManager(goff_t addr, size_t size) : list(new Area()) {
        list->addr = addr;
        list->size = size;
        list->next = nullptr;
    }

    AreaManager(const AreaManager &) = delete;
    AreaManager &operator=(const AreaManager &) = delete;

    /**
     * Destroys this map
     */
    ~AreaManager() {
        for(auto *a = list; a != nullptr;) {
            auto *n = a->next;
            delete a;
            a = n;
        }
        list = nullptr;
    }

    /**
     * Allocates an area that is <size> bytes large.
     *
     * @param size the size of the area
     * @param align the desired alignment
     * @return the address, if space was found, 0 otherwise
     */
    goff_t allocate(size_t size, size_t align = 1) {
        Area *a;
        Area *p = nullptr;
        for(a = list; a != nullptr; p = a, a = a->next) {
            size_t diff = m3::Math::round_up(a->addr, static_cast<goff_t>(align)) - a->addr;
            if(a->size > diff && a->size - diff >= size)
                break;
        }
        if(a == nullptr)
            return 0;

        // if we need to do some alignment, create a new area in front of a
        size_t diff = m3::Math::round_up(a->addr, static_cast<goff_t>(align)) - a->addr;
        if(diff) {
            auto *n = new Area();
            n->addr = a->addr;
            n->size = diff;
            if(p)
                p->next = n;
            else
                list = n;
            n->next = a;

            a->addr += diff;
            a->size -= diff;
            p = n;
        }

        // take it from the front
        goff_t res = a->addr;
        a->size -= size;
        a->addr += size;
        // if the area is empty now, remove it
        if(a->size == 0) {
            if(p)
                p->next = a->next;
            else
                list = a->next;
            delete a;
        }
        return res;
    }

    /**
     * Frees the area at <addr> with <size> bytes.
     *
     * @param addr the address of the area
     * @param size the size of the area
     */
    void free(goff_t addr, size_t size) {
        // find the area behind ours
        Area *n, *p = nullptr;
        for(n = list; n != nullptr && addr > n->addr; p = n, n = n->next)
            ;

        // merge with prev and next
        if(p && p->addr + p->size == addr && n && addr + size == n->addr) {
            p->size += size + n->size;
            p->next = n->next;
            delete n;
        }
        // merge with prev
        else if(p && p->addr + p->size == addr) {
            p->size += size;
        }
        // merge with next
        else if(n && addr + size == n->addr) {
            n->addr -= size;
            n->size += size;
        }
        // create new area between them
        else {
            auto *a = new Area();
            a->addr = addr;
            a->size = size;
            if(p)
                p->next = a;
            else
                list = a;
            a->next = n;
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
            format_to(os, "\t@ {:p}, {} KiB\n"_cf, a->addr, a->size / 1024);
    }

private:
    Area *list;
};

}
