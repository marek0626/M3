/*
 * Copyright (C) 2024-2024, Stephan Gerhold <stephan@gerhold.net>
 * This file is part of M4 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M4 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 3 as
 * published by the Free Software Foundation.
 *
 * M4 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 3 for more details.
 */

MEMORY
{
#if __riscv_xlen == 32
    STACK : ORIGIN = 0x0, LENGTH = 8K
    BSS : ORIGIN = 0x2000, LENGTH = 4K
    RAM : ORIGIN = 0x3000, LENGTH = 128K
#else
    STACK : ORIGIN = 0x10000000, LENGTH = 8K
    BSS : ORIGIN = 0x10002000, LENGTH = 4K
    RAM : ORIGIN = 0x10003000, LENGTH = 84K
#endif
}

REGION_ALIAS("REGION_TEXT", RAM);
REGION_ALIAS("REGION_RODATA", RAM);
REGION_ALIAS("REGION_DATA", BSS); /* Copied from RAM at startup */
REGION_ALIAS("REGION_BSS", BSS);
REGION_ALIAS("REGION_HEAP", BSS);
REGION_ALIAS("REGION_STACK", STACK);

/*
 * No secrets should end up in the BSS, so it should be sufficient to clear
 * the stack (_sstack). However, the BSS is quite small, so we just clear
 * everything to be sure.
 */
PROVIDE(_eclear = _ebss);
