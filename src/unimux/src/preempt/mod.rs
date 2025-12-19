mod activities;
mod cureq;
mod entry;
mod state;
mod timer;
mod tmcalls;

use base::tcu;
use mux::helper;

extern "C" {
    fn sleep_once();
}

pub fn init() {
    activities::init();
    entry::init();
}

pub fn user_id() -> Option<tcu::ActId> {
    if activities::user_is_some() {
        Some(activities::user().id() as tcu::ActId)
    }
    else {
        None
    }
}

pub fn user_init(id: u64) {
    activities::set_user(id);
}

pub fn user_start() {
    activities::user().start();
}

pub fn user_block() {
    activities::user().set_blocked(true);
}

pub fn user_ready_or_sleep() -> bool {
    if activities::user_is_some() && activities::user().is_ready() {
        return true;
    }

    unsafe {
        sleep_once();
    }
    false
}

pub fn handle_sidecalls<F: FnMut() -> bool>(mut handle: F) {
    let mut our = activities::our();
    let _cmd_saved = helper::TCUGuard::new();

    loop {
        // change to our activity
        let old_act = tcu::TCU::xchg_activity(our.activity_reg()).unwrap();
        if let Some(old) = activities::try_cur() {
            activities::get_mut(old).unwrap().set_activity_reg(old_act);
        }

        handle();

        // change back to old activity
        let new_act = match activities::try_cur() {
            Some(cur) if cur != (old_act & 0xFFFF) => {
                activities::get_mut(cur).unwrap().activity_reg()
            },
            _ => old_act,
        };
        our.set_activity_reg(tcu::TCU::xchg_activity(new_act).unwrap());
        // if no events arrived in the meantime, we're done
        if !our.has_msgs() {
            break;
        }
    }
}

pub fn run_to_completion() -> ! {
    // not used
    unreachable!();
}
