/*
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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

use core::arch::asm;

use base::io::LogFlags;
use base::log;

use crate::{CtxData, LayerCtx};

#[macro_export]
macro_rules! generate_entry {
    () => {
        #[cfg(target_arch = "riscv32")]
        core::arch::global_asm!(
            ".section .init.reset, \"ax\"",
            ".global _reset",
            "_reset:",
            // Clear bss
            "   la      x3, __bss_start",
            "   la      x4, __bss_end",
            "1: sw      zero, 0(x3)",
            "   addi    x3, x3, 4",
            "   bne     x3, x4, 1b",
            // jump to actual entry
            "   j       _start",
        );

        #[cfg(target_arch = "riscv64")]
        core::arch::global_asm!(
            ".section .init.reset, \"ax\"",
            ".global _reset",
            "_reset:",
            // Clear bss
            "   la      x3, __bss_start",
            "   la      x4, __bss_end",
            "1: sd      zero, 0(x3)",
            "   addi    x3, x3, 8",
            "   bne     x3, x4, 1b",
            // jump to actual entry
            "   j       _start",
        );
    };
}

#[cfg(any(not(M3_TARGET = "gem5"), target_arch = "riscv32"))]
unsafe fn prepare_switch<Data: CtxData>(ctx: &LayerCtx<Data>) -> usize {
    let entry = ctx.entry_addr;
    asm!(
        // Clear context page
        "   mv      x4, {ctx_off}",
        "1: sw      zero, 0(x4)",
        "   addi    x4, x4, 4",
        "   bne     x4, {ctx_end}, 1b",

        // Copy the context to the beginning of memory
        "1: lw      x4, 0({nctx_off})",
        "   addi    {nctx_off}, {nctx_off}, 4",
        "   sw      x4, 0({ctx_off})",
        "   addi    {ctx_off}, {ctx_off}, 4",
        "   bne     {ctx_off}, {nctx_end}, 1b",

        // Clear stack
        "   la      x3, baremetal_stack_start",
        "   la      x4, baremetal_stack",
        "1: sw      zero, 0(x3)",
        "   addi    x3, x3, 4",
        "   bne     x3, x4, 1b",

        // Clear data and bss
        "   la      x3, __data_start",
        "   la      x4, __bss_end",
        "1: sw      zero, 0(x3)",
        "   addi    x3, x3, 4",
        "   bne     x3, x4, 1b",

        ctx_off = in(reg) LayerCtx::<Data>::CTX_OFFSET,
        ctx_end = in(reg) LayerCtx::<Data>::CTX_OFFSET + base::cfg::PAGE_SIZE,
        nctx_off = in(reg) ctx,
        nctx_end = in(reg) LayerCtx::<Data>::CTX_OFFSET + core::mem::size_of::<LayerCtx<Data>>(),
    );
    entry
}

#[cfg(all(M3_TARGET = "gem5", target_arch = "riscv64"))]
unsafe fn prepare_switch<Data: CtxData>(ctx: &LayerCtx<Data>) -> usize {
    let entry = ctx.entry_addr;
    asm!(
        // Clear context page
        "   mv      x4, {ctx_off}",
        "1: sd      zero, 0(x4)",
        "   addi    x4, x4, 8",
        "   bne     x4, {ctx_end}, 1b",

        // Copy the new context
        "1: ld      x4, 0({nctx_off})",
        "   addi    {nctx_off}, {nctx_off}, 8",
        "   sd      x4, 0({ctx_off})",
        "   addi    {ctx_off}, {ctx_off}, 8",
        "   bne     {ctx_off}, {nctx_end}, 1b",

        // Clear stack
        "   la      x3, baremetal_stack_start",
        "   la      x4, baremetal_stack",
        "1: sd      zero, 0(x3)",
        "   addi    x3, x3, 8",
        "   bne     x3, x4, 1b",

        // Clear data and bss
        "   la      x3, __data_start",
        "   la      x4, __bss_end",
        "1: sd      zero, 0(x3)",
        "   addi    x3, x3, 8",
        "   bne     x3, x4, 1b",

        ctx_off = in(reg) LayerCtx::<Data>::CTX_OFFSET,
        ctx_end = in(reg) LayerCtx::<Data>::CTX_OFFSET + base::cfg::PAGE_SIZE,
        nctx_off = in(reg) ctx,
        nctx_end = in(reg) LayerCtx::<Data>::CTX_OFFSET + core::mem::size_of::<LayerCtx<Data>>(),
    );
    entry
}

pub(crate) unsafe fn switch<Data: CtxData>(ctx: LayerCtx<Data>) -> ! {
    log!(
        LogFlags::RoTBoot,
        "Jumping to next layer @ {:#x}",
        ctx.entry_addr
    );
    let entry = prepare_switch(&ctx);
    asm!(
        // Clear registers
        //" li      x1, 0", // Contains entry address
        "   li      x2, 0",
        "   li      x3, 0",
        "   li      x4, 0",
        "   li      x5, 0",
        "   li      x6, 0",
        "   li      x7, 0",
        "   li      x8, 0",
        "   li      x9, 0",
        "   li      x10, 0",
        "   li      x11, 0",
        "   li      x12, 0",
        "   li      x13, 0",
        "   li      x14, 0",
        "   li      x15, 0",
        "   li      x16, 0",
        "   li      x17, 0",
        "   li      x18, 0",
        "   li      x19, 0",
        "   li      x20, 0",
        "   li      x21, 0",
        "   li      x22, 0",
        "   li      x23, 0",
        "   li      x24, 0",
        "   li      x25, 0",
        "   li      x26, 0",
        "   li      x27, 0",
        "   li      x28, 0",
        "   li      x29, 0",
        "   li      x30, 0",
        "   li      x31, 0",
        // fence instruction is not supported on hw23 (and apparently unnecessary on gem5)
        // "   fence",

        // Jump to the new entry point
        "   ret",

        in("x1") entry, // ra
        options(noreturn)
    )
}

pub(crate) unsafe fn sleep<Data: CtxData>(ctx: &LayerCtx<Data>) -> ! {
    log!(
        LogFlags::RoTBoot,
        "Sleeping until external reset to next layer @ {:#x}",
        ctx.entry_addr
    );
    loop {
        asm!(
            // Dummy usage to make sure context is not discarded
            "/* {ctx_pos} */",
            "wfi",
            ctx_pos = in(reg) ctx
        );
    }
}
