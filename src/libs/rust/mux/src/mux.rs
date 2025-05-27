#![no_std]
#![allow(warnings)]

use base::cell::Ref;
use base::env;
use base::kif;
use base::mem::VirtAddr;

pub mod helper;
pub mod sendqueue;
pub mod sidecalls;

pub struct TMEnv {
    pub tile_id: u64,
    pub org_tile_desc: kif::TileDesc,
    pub tile_desc: kif::TileDesc,
    pub platform: env::Platform,
}

pub fn init(pex_env: Ref<'static, TMEnv>) {
    helper::init(pex_env.tile_desc.has_virtmem());
    sidecalls::init(pex_env.tile_desc.rbuf_mux_space().0);
}
