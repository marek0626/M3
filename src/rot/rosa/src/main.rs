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

#![no_std]
#![no_main]

use core::cmp::min;
use riscv_rt::entry;

use base::cell::StaticUnsafeCell;
use base::errors::Error;
use base::io::LogFlags;
use base::mem::{self, AlignedBuf, GlobAddr, GlobOff};
use base::tcu::{self, EpId, TCU};
use base::{cfg, log, machine};
#[allow(unused_imports)]
use lang as _;
use rot::CtxData;

mod idxtile;
mod stage1;
mod stage2;

pub const MEM_EP: EpId = tcu::FIRST_USER_EP + 0;
pub const COPY_EP: EpId = tcu::FIRST_USER_EP + 1;
pub const ENV_EP: EpId = tcu::FIRST_USER_EP + 2;
pub const SELF_EP: EpId = tcu::FIRST_USER_EP + 3;

#[repr(C)]
#[derive(Debug)]
pub struct RosaPrivateCtx {
    next: rot::RosaCtx,
    our_tile: idxtile::IndexedTile,
    kernel_tile: idxtile::IndexedTile,
    kernel_tile_desc: u64,
    kenv_addr: GlobAddr,
    root_tile_id: u64,
}

impl CtxData for RosaPrivateCtx {
    // Should be different from RosaCtx::MAGIC
    const MAGIC: rot::Magic = rot::encode_magic(b"RosaCtx", 0);
}

pub type RosaPrivateLayerCtx = rot::LayerCtx<RosaPrivateCtx>;

const HEAP_SIZE: usize = 32 * 1024;
const COPY_BUF_SIZE: usize = 4 * 1024;

pub const EP_REGS_SIZE: usize = tcu::EP_REGS * mem::size_of::<tcu::Reg>();
pub const EPS_PER_PAGE: usize = cfg::PAGE_SIZE / EP_REGS_SIZE;

// unsafe could be avoided using StaticRefCell but this would waste 4 KiB
// of memory because of the required page-alignment
pub static COPY_BUF: StaticUnsafeCell<AlignedBuf<COPY_BUF_SIZE>> =
    StaticUnsafeCell::new(AlignedBuf::new_zeroed());

#[link_section = ".bss"] // Avoid taking up space in the binary (.rodata)
static EMPTY_BUF: AlignedBuf<COPY_BUF_SIZE> = AlignedBuf::new_zeroed();

static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

// Used without locking at the moment because ROSA is single-threaded and does not use interrupts
#[global_allocator]
static ALLOCATOR: talc::Talck<talc::locking::AssumeUnlockable, talc::ErrOnOom> =
    talc::Talc::new(talc::ErrOnOom).lock();

pub fn clear_mem(mut off: GlobOff, mut size: usize) -> Result<(), Error> {
    while size > 0 {
        let len = min(size, COPY_BUF_SIZE);
        TCU::write(MEM_EP, EMPTY_BUF.as_ptr(), len, off)?;
        off += len as GlobOff;
        size -= len;
    }
    Ok(())
}

pub fn config_local_ep<CFG>(ep: tcu::EpId, cfg: CFG)
where
    CFG: FnOnce(&mut [tcu::Reg]),
{
    let mut regs = [0; tcu::EP_REGS];
    cfg(&mut regs);
    TCU::set_ep_regs(ep, &regs);
}

#[no_mangle]
pub extern "C" fn exit(_code: i32) -> ! {
    log!(LogFlags::Info, "Shutting down");
    machine::shutdown();
}

#[entry]
fn main() -> ! {
    // Initialize heap allocator
    unsafe { ALLOCATOR.lock().claim(core::ptr::addr_of!(HEAP).into()) }.unwrap();

    let ctx = unsafe { rot::LayerCtx::<()>::get() };
    if ctx.magic == RosaPrivateCtx::MAGIC {
        stage2::main();
    }
    else {
        stage1::main()
    }
}
