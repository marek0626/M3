/*
 * Copyright (C) 2015-2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
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

#include <m3/session/Network.h>

#define RUN_SUITE(name)                            \
    m3::println("Running testsuite {}"_cf, #name); \
    name();                                        \
    m3::println();

#define RUN_TEST(name)                                        \
    m3::println("Testing \"{}\" in {}:"_cf, #name, __FILE__); \
    name();                                                   \
    m3::println();

extern m3::IpAddr NET0_IP;
extern m3::IpAddr NET1_IP;
extern m3::IpAddr DEST_IP;

void tudp();
void ttcp();
