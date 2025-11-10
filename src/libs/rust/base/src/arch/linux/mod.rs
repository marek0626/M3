pub mod ioctl;
pub mod mmap;

use std::fs::{File, OpenOptions};
use std::mem::MaybeUninit;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

use crate::cell::LazyStaticRefCell;
use crate::cfg;
use crate::env;
use crate::kif::Perm;
use crate::tcu;
use crate::time::TimeDuration;

static TCU_DEV: LazyStaticRefCell<File> = LazyStaticRefCell::default();

pub fn tcu_fd() -> libc::c_int {
    TCU_DEV.borrow().as_raw_fd()
}

pub fn init_fd() {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/tcu")
        .expect("Unable to open /dev/tcu");
    TCU_DEV.set(file);
}

pub fn wait_msg(timeout: Option<TimeDuration>) {
    let timeout = timeout.map(|d| d.as_nanos()).unwrap_or(0);
    ioctl::wait_msg(timeout as usize);
}

pub fn init_env() {
    mmap::mmap_tcu(
        tcu_fd(),
        cfg::ENV_START,
        cfg::ENV_SIZE,
        mmap::MemType::Environment,
        Perm::RW,
    )
    .expect("Unable to map environment");
}

extern "C" fn handle_sigsegv(
    _sig: libc::c_int,
    sig_info: *mut libc::siginfo_t,
    _ucontext_void: *mut libc::c_void,
) {
    if sig_info.is_null() {
        unsafe {
            libc::_exit(1);
        }
    }
    let sig_info = unsafe { *sig_info };
    let si_addr = unsafe { sig_info.si_addr() };
    if si_addr.is_null() {
        unsafe {
            libc::_exit(1);
        }
    }
    let si_addr = si_addr as usize as u64;
    if si_addr >= tcu::MMIO_ADDR.as_raw()
        && si_addr < (tcu::MMIO_ADDR.as_raw() + tcu::MMIO_SIZE as u64)
    {
        mmap::mmap_tcu(
            tcu_fd(),
            tcu::MMIO_ADDR,
            tcu::MMIO_SIZE,
            mmap::MemType::TCU,
            Perm::RW,
        )
        .expect("Unable to map TCU MMIO region");
    }
    else {
        unsafe {
            libc::_exit(1);
        }
    }
}

fn install_sigsegv_handler() {
    unsafe {
        let mut mask: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
        _ = libc::sigemptyset(mask.as_mut_ptr());

        let new_action = libc::sigaction {
            sa_sigaction: handle_sigsegv as *const fn() as *const libc::c_void as usize,
            sa_mask: mask.assume_init(),
            sa_flags: libc::SA_SIGINFO,
            sa_restorer: None,
        };

        let mut old_action: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();

        libc::sigaction(libc::SIGSEGV, &new_action, old_action.as_mut_ptr());
        libc::sigaction(libc::SIGBUS, &new_action, old_action.as_mut_ptr());
    }
}

pub fn init() {
    init_fd();

    init_env();

    install_sigsegv_handler();

    #[cfg(not(M3_TARGET = "hw23"))]
    mmap::mmap_tcu(
        tcu_fd(),
        tcu::MMIO_EPS_ADDR,
        tcu::TCU::endpoints_size(),
        mmap::MemType::TCUEPs,
        Perm::R,
    )
    .expect("Unable to map TCU-EPs MMIO region");

    let (rbuf_virt_addr, rbuf_size) = env::boot().tile_desc().rbuf_std_space();
    mmap::mmap_tcu(
        tcu_fd(),
        rbuf_virt_addr,
        rbuf_size,
        mmap::MemType::StdRecvBuf,
        Perm::R,
    )
    .expect("Unable to map standard receive buffer");
}
