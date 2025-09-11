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

#include <m3/Test.h>

#include <string.h>
#include <sys/random.h>
#include <sys/utsname.h>

#include "../libctest.h"

using namespace m3;

static void test_uname() {
    struct utsname buf;
    WVASSERTEQ(uname(&buf), 0);
    WVASSERTEQ(strcmp(buf.sysname, "M3"), 0);
#if defined(__riscv)
    WVASSERTEQ(strcmp(buf.machine, "RISC-V"), 0);
#elif defined(__x86_64__)
    WVASSERTEQ(strcmp(buf.machine, "x86-64"), 0);
#else
#    error "Unsupported ISA"
#endif
}

static void test_getrandom() {
    char buf[16];
    WVASSERTEQ(getrandom(buf, sizeof(buf), 0), static_cast<ssize_t>(sizeof(buf)));
}

void tmisc() {
    RUN_TEST(test_uname);
    RUN_TEST(test_getrandom);
}
