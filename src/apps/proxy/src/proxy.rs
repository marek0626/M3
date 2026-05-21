#![no_std]
use m3::{errors::Error, println};
#[unsafe(no_mangle)]
pub fn main() -> Result<(), Error> {
    println!("=======================");
    println!("Proxy booting up");
    println!("=======================");
    Ok(())
}
