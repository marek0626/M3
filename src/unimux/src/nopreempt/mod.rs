use base::cell::StaticCell;

static STARTED: StaticCell<bool> = StaticCell::new(false);

pub fn init() {
}

pub fn user_init(_id: u64) {
}

pub fn user_start() {
    STARTED.set(true);
}

pub fn user_block() {
}

pub fn user_ready_or_sleep() -> bool {
    STARTED.get()
}

pub fn handle_sidecalls<F: FnMut() -> bool>(mut handle: F) {
    loop {
        if !handle() {
            break;
        }
    }
}
