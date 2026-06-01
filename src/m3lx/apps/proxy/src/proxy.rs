#![no_std]
extern crate m3core as m3;

use m3::errors::Error;
use m3::println;
use m3::server::{RequestHandler, RequestSession, Server, ServerSession};

// Defining custom session state
struct ProxySession {
    serv: ServerSession,
}

// Implemenation for the request state which is called automatically
impl RequestSession for ProxySession {
    fn new(serv: ServerSession, arg: &str) -> Result<Self, Error> {
        println!(">>> PROXY: Received a new session connection! <<<");
        println!(">>> PROXY: Session args: {} <<<", arg);
        Ok(ProxySession { serv })
    }
}

pub fn main() -> Result<(), Error> {
    m3::env::init();

    println!("======================================");
    println!(" M3 Proxy Server Booting Up! ");
    println!("======================================");

    // Initialize the Request Handler
    let mut reqhdl = RequestHandler::<ProxySession, usize>::new()?;

    // Create the Server
    // THIS is the line that calls `reg_service` and unblocks netechoserver
    let mut srv = Server::new("proxy", &mut reqhdl)?;

    println!("Proxy successfully announced 'proxy' service and is waiting...");

    // This automatically blocks, listens to the gate, and handles connections
    reqhdl.run(&mut srv)?;

    Ok(())
}
