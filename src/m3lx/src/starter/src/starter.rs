use util::mmap::{MemType, Mmap};

use base::cfg;
use base::env;
use base::linux::{ioctl, mmap};
use base::tcu::{self};

fn main() -> Result<(), std::io::Error> {
    ioctl::init();
    mmap::init();

    // these need to stay in scope so that the mmaped areas stay alive
    let _tcu_mmap = Mmap::new("/dev/tcu", tcu::MMIO_ADDR, MemType::TCU, tcu::MMIO_SIZE)?;
    let _env_mmap = Mmap::new(
        "/dev/tcu",
        cfg::ENV_START,
        MemType::Environment,
        cfg::ENV_SIZE,
    )?;

    ioctl::register_act();

    if unsafe { libc::fork() } == 0 {
        let name = env::args().next().unwrap();
        let mut bytes = [0u8; 1024];
        let mut argv: Vec<*const u8> = vec![];
        let mut pos = 0;
        for arg in env::args() {
            // store null-terminated argument into bytes array
            bytes[pos..pos + arg.len()].copy_from_slice(arg.as_bytes());
            bytes[pos + arg.len()] = b'\0';
            // store pointer in argv array
            unsafe {
                argv.push(bytes.as_ptr().add(pos));
            }
            pos += arg.len() + 1;
        }
        argv.push(std::ptr::null());

        println!(
            "Running {} {:?} in process {}",
            name,
            env::args().skip(1).collect::<Vec<_>>(),
            unsafe { libc::getpid() },
        );

        let res = unsafe { libc::execvp(name.as_ptr(), argv.as_ptr()) };
        if res != 0 {
            println!("execvp failed: {}", res);
        }
    }
    else {
        let mut status = 0;
        let pid = unsafe { libc::wait(&mut status as *mut _) };
        println!("Process {} exited with status {}", pid, status);
    }

    ioctl::unregister_act();

    Ok(())
}
