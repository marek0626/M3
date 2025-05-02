/*
 * Copyright (C) 2025 Nils Asmussen, Barkhausen Institut
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

#include <base/Common.h>
#include <base/mem/AreaManager.h>
#include <base/mem/InPlaceAreaManager.h>

#include <m3/Test.h>

#include "../unittests.h"

using namespace m3;

template<class MNG>
static void test_free_ooo(MNG &mng) {
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);

    auto res1 = mng.allocate(0x200);
    auto res2 = mng.allocate(0x400);
    auto res3 = mng.allocate(0x800);
    WVASSERT(mng.allocate(0x300) == 0);
    WVASSERTEQ(mng.size().first, 0x200U);

    mng.free(res2, 0x400);
    mng.free(res3, 0x800);
    mng.free(res1, 0x200);
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);
}

template<class MNG>
static void test_free_inorder(MNG &mng) {
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);

    auto res1 = mng.allocate(0x200);
    auto res2 = mng.allocate(0x400);
    auto res3 = mng.allocate(0x800);
    WVASSERTEQ(mng.size().first, 0x200U);

    mng.free(res1, 0x200);
    mng.free(res2, 0x400);
    mng.free(res3, 0x800);
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);
}

template<class MNG>
static void test_free_revorder(MNG &mng) {
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);

    auto res1 = mng.allocate(0x200);
    auto res2 = mng.allocate(0x400);
    auto res3 = mng.allocate(0x800);
    WVASSERTEQ(mng.size().first, 0x200U);

    mng.free(res3, 0x800);
    mng.free(res2, 0x400);
    mng.free(res1, 0x200);
    WVASSERTEQ(mng.size().first, 0x1000U);
    WVASSERTEQ(mng.size().second, 1U);
}

static void areamng() {
    AreaManager mng(0x40000, 0x1000);
    test_free_ooo(mng);
    test_free_inorder(mng);
    test_free_revorder(mng);
}

static void areamng_aligns() {
    AreaManager mng(0x40000, 0x10000);
    WVASSERTEQ(mng.size().first, 0x10000U);
    WVASSERTEQ(mng.size().second, 1U);

    auto res1 = mng.allocate(0x200, 0x1000);
    WVASSERT((res1 & (0x1000 - 1)) == 0);
    WVASSERTEQ(mng.size().second, 1U);
    auto res2 = mng.allocate(0x400, 0x2000);
    WVASSERT((res1 & (0x2000 - 1)) == 0);
    WVASSERTEQ(mng.size().second, 2U);
    auto res3 = mng.allocate(0x800, 0x4000);
    WVASSERT((res1 & (0x4000 - 1)) == 0);
    WVASSERTEQ(mng.size().first, 0x10000U - (0x200 + 0x400 + 0x800));
    WVASSERTEQ(mng.size().second, 3U);

    mng.free(res3, 0x800);
    mng.free(res2, 0x400);
    mng.free(res1, 0x200);
    WVASSERTEQ(mng.size().first, 0x10000U);
    WVASSERTEQ(mng.size().second, 1U);
}

static void inplace_areamng() {
    std::unique_ptr<uint8_t[]> mem(new uint8_t[0x1000]());
    InPlaceAreaManager mng(mem.get(), 0x1000);

    test_free_ooo(mng);
    test_free_inorder(mng);
    test_free_revorder(mng);
}

void tareamng() {
    RUN_TEST(areamng);
    RUN_TEST(areamng_aligns);
    RUN_TEST(inplace_areamng);
}
