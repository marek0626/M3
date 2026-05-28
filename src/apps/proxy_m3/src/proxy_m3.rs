#![no_std]
use m3::{
    com::{RecvGate, SendGate},
    errors::Error,
    println, send_recv,
};
#[unsafe(no_mangle)]
pub fn main() -> Result<(), Error> {
    println!("=======================");
    println!("Proxy booting up");
    println!("=======================");
    let sgate = SendGate::new_named("chan")?;
    println!("Proxy connected to M3Linux rgate");

    let val: u32 = 99;
    println!("Proxy sending test value {}", val);

    send_recv!(&sgate, RecvGate::def(), val)?;
    println!("Proxy received acknowledgment from M3Linux");
    Ok(())
}
