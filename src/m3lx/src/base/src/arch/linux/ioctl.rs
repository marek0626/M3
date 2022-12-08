use libc;
use std::os::unix::prelude::AsRawFd;

// this is defined in linux/drivers/tcu/tcu.cc (and the right value will be printed on driver initialization during boot time)
const IOCTL_RGSTR_ACT: u64 = 0x00007101;
const IOCTL_TLB_INSRT: u64 = 0x40087102;
const IOCTL_UNREG_ACT: u64 = 0x00007103;

const TCU_DEV: &str = "/dev/tcu";

fn ioctl(magic_number: u64) {
    let tcu_dev = std::fs::File::open(TCU_DEV).expect("could not open ioctl device");
    unsafe {
        let res = libc::ioctl(tcu_dev.as_raw_fd(), magic_number);
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl call {} failed", magic_number);
        }
    }
}

fn ioctl_write<T>(magic_number: u64, arg: T) {
    let tcu_dev = std::fs::File::open(TCU_DEV).expect("could not open ioctl device");
    unsafe {
        let res = libc::ioctl(tcu_dev.as_raw_fd(), magic_number, &arg as *const _);
        if res != 0 {
            libc::perror(0 as *const u8);
            panic!("ioctl call {} failed", magic_number);
        }
    }
}

pub fn register_act() {
    ioctl(IOCTL_RGSTR_ACT);
}

#[repr(C)]
struct TlbInsert {
    virt: u64,
    perm: u8,
}

pub fn tlb_insert_addr(virt: u64, perm: u8) {
    let arg = TlbInsert {
        virt,
        perm,
    };
    ioctl_write(IOCTL_TLB_INSRT, arg);
}

pub fn unregister_act() {
    ioctl(IOCTL_UNREG_ACT);
}
