#![no_std]
extern crate m3core as m3;

use m3::cap::SelSpace;
use m3::errors::Error;
use m3::println;
use m3::server::{ExcType, RequestHandler, RequestSession, Server, ServerSession};

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

    fn close(
        &mut self,
        _cli: &mut m3::server::ClientManager<Self>,
        _sid: m3::server::SessId,
        _sub_ids: &mut m3::vec::Vec<m3::server::SessId>,
    ) where
        Self: Sized,
    {
        println!("The close function was called");
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
    reqhdl.reg_cap_handler(4, ExcType::Obt(2), |_clients, _crt, _sid, xchg| {
        println!("Proxy caught create!");
        let sock_ty: u8 = xchg.in_args().pop()?;
        let protocol: u8 = xchg.in_args().pop()?;
        let rbuf_size: usize = xchg.in_args().pop()?;
        let rbuf_slots: usize = xchg.in_args().pop()?;
        let sbuf_size: usize = xchg.in_args().pop()?;
        let sbuf_slots: usize = xchg.in_args().pop()?;
        println!(
            ">>> PROXY: Socket requested - Type: {}, Protocol: {}, RBuf: {}x{}, SBuf: {}x{} <<<",
            sock_ty, protocol, rbuf_size, rbuf_slots, sbuf_size, sbuf_slots
        );

        let dummy_sd: usize = 42;
        xchg.out_args().push(dummy_sd);

        let sels = SelSpace::get().alloc_sels(2);
        xchg.out_caps(sels);

        Ok(())
    });

    // This automatically blocks, listens to the gate, and handles connections
    reqhdl.run(&mut srv)?;

    Ok(())
}
