use base::cell::StaticCell;
use base::tcu;

static STARTED: StaticCell<bool> = StaticCell::new(false);
static ACT_ID: StaticCell<Option<tcu::ActId>> = StaticCell::new(None);

pub fn init() {
}

pub fn user_id() -> Option<tcu::ActId> {
    ACT_ID.get()
}

pub fn user_init(id: u64) {
    ACT_ID.set(Some(id as tcu::ActId));
}

pub fn user_start() {
    STARTED.set(true);
}

pub fn user_block() {
}

pub fn user_ready_or_sleep() -> bool {
    crate::sidecalls::check();
    STARTED.get()
}

pub fn handle_sidecalls<F: FnMut() -> bool>(mut handle: F) {
    loop {
        if !handle() {
            break;
        }
    }
}

pub fn run_to_completion() -> ! {
    // pass correct tile description to user (see above)
    crate::app_env().boot.tile_desc = crate::pex_env().tile_desc.value();

    // run application
    unsafe {
        crate::env_run();
    }
}
