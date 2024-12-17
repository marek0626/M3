#
# Copyright (C) 2015-2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
# Economic rights: Technische Universitaet Dresden (Germany)
#
# Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
#
# This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
#
# M3 is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 2 as
# published by the Free Software Foundation.
#
# M3 is distributed in the hope that it will be useful, but
# WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
# General Public License version 2 for more details.
#

#include <base/Asm.h>
#include <base/Config.h>

.extern init
.extern isr_stack

.section .init

BEGIN_FUNC(_start)
    la      sp, isr_stack
    call    init

    // return to user mode
    li      a0, 1 << 8
    csrc    sstatus, a0
    // enable interrupts
    li      a0, 1 << 5
    csrs    sstatus, a0

    // jump to sleep function
    la      a0, sleep
    csrw    sepc, a0

    // go!
    sret
END_FUNC(_start)

// fn _shutdown()
BEGIN_FUNC(_shutdown)
    // remove potentially pending interrupts
    csrw    sip, x0
    // disable supervisor interrupts
    li      t0, 1 << 5
    csrc    sstatus, t0
1:
    wfi
    j       1b
END_FUNC(_shutdown)


.section .user_text

BEGIN_FUNC(sleep)
    la      a0, ENV_START
    // load platform from environment
    lmw     a0, 0(a0)
    // skip wfi on hw (TODO why does that not work?)
1:  bnez    a0, 2f
    wfi
2:
    j       1b
END_FUNC(sleep)

BEGIN_FUNC(sleep_once)
    li     a0, 2
    csrs   sstatus, a0
    la      a0, ENV_START
    // load platform from environment
    lmw     a0, 0(a0)
    // skip wfi on hw (TODO why does that not work?)
1:  bnez    a0, 2f
    wfi
2:	li a0, 0
    csrs sstatus, a0
    ret
END_FUNC(sleep_once)
