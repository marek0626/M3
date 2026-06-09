#![no_std]
extern crate m3core as m3;

use m3::cap::{CapFlags, SelSpace};
use m3::com::{RGateArgs, RecvCap, RecvGate, SGateArgs, SendCap, SendGate, opcodes};
use m3::errors::Error;
use m3::net::NetEventChannel;
use m3::println;
use m3::server::{ExcType, RequestHandler, RequestSession, Server, ServerSession};
use m3::util::math::{self, next_log2};

// Defining custom session state
struct ProxySession {
    serv: ServerSession,
    gates: Option<(RecvGate, SendCap)>,
}

// Implemenation for the request state which is called automatically
impl RequestSession for ProxySession {
    fn new(serv: ServerSession, arg: &str) -> Result<Self, Error> {
        println!(">>> PROXY: Received a new session connection! <<<");
        println!(">>> PROXY: Session args: {} <<<", arg);
        Ok(ProxySession { serv, gates: None })
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
    reqhdl.reg_cap_handler(opcodes::Net::Create as usize, ExcType::Obt(2), |_clients, _crt, _sid, xchg| {
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
        let caps = sels.start();

        // The proxy gives the recvcap to the client at slot 0
        let client_rgate = RecvCap::new_with(
            RGateArgs::default()
                .sel(caps + 0)
                .msg_order(math::next_log2(2048))
                .order(math::next_log2(2048 * 4))
                .flags(CapFlags::KEEP_CAP))?;
        println!("Client rgate went through");

        // The proxy keeps a sgate pointing to it
        let proxy_sgate = SendCap::new_with(SGateArgs::new(&client_rgate).credits(4))?;
        println!("proxy sgate");

        // The proxy creates and keeps an rgate
        let proxy_rgate = RecvGate::new(math::next_log2(128), math::next_log2(32))?;
        println!("proxy rgate");

        // The proxy points the clients sgate to it
        let client_sgate = SendCap::new_with(SGateArgs::new(&proxy_rgate)
            .sel(caps + 1)
            .credits(4)
            .flags(CapFlags::KEEP_CAP))?;
        println!("client sgate");

        // send the ends to the client
        xchg.out_caps(sels);

        println!("send cap to client");

        // save the proxy's ends
        let sess = _clients.get_mut(_sid).unwrap();
        sess.gates = Some((proxy_rgate, proxy_sgate));
        println!("saved gates");

        Ok(())
    });

    reqhdl.reg_msg_handler(opcodes::Net::GetIP as usize, |_sess, msg| {
        let dummy_addr: u32 = 0;
        m3::reply_vmsg!(msg, m3::errors::Code::Success, dummy_addr);
        Ok(())
    });

    reqhdl.reg_msg_handler(opcodes::Net::Bind as usize, |_sess, msg| {
        let sd: usize = msg.pop()?;
        let port: u16 = msg.pop()?;

        println!(">>> Proxy: Bind requested on SD: {}, Port: {}", sd, port);

        let dummy_ip: u32 = 0;

        m3::reply_vmsg!(msg, m3::errors::Code::Success, dummy_ip, port);

        Ok(())
    });
    reqhdl.reg_msg_handler(opcodes::Net::GetNameSrv as usize, |_sess, msg| {
        println!(">>> Proxy: GetNameServer requested");

        let dummy_addr: u32 = 0;

        m3::reply_vmsg!(msg, m3::errors::Code::Success, dummy_addr);

        Ok(())
    });
    reqhdl.reg_msg_handler(opcodes::Net::Listen as usize, |_sess, msg| {
        let sd: usize = msg.pop()?;
        let port: u16 = msg.pop()?;

        println!(">>> Proxy: Listen requested on SD: {}, Port: {}", sd, port);

        let dummy_addr: u32 = 0;

        m3::reply_vmsg!(msg, m3::errors::Code::Success, dummy_addr);

        Ok(())
    });
    reqhdl.reg_msg_handler(opcodes::Net::Connect as usize, |_sess, msg| {
        let sd: usize = msg.pop()?;
        let addr: u32 = msg.pop()?;
        let port: u16 = msg.pop()?;

        println!(
            ">>> Proxy: Connect requested on SD: {}, Port: {}, Addr: {}",
            sd, port, addr
        );
        let dummy_addr: u32 = 0;
        let dummy_port: u16 = 0;

        m3::reply_vmsg!(msg, m3::errors::Code::Success, dummy_addr, dummy_port);
        Ok(())
    });
    reqhdl.reg_msg_handler(opcodes::Net::Abort as usize, |_sess, msg| {
        let sd: usize = msg.pop()?;
        let remove: bool = msg.pop()?;

        // m3::log!();
        println!(
            ">>> Proxy: Abort requested on SD: {}, with Remove: {}",
            sd, remove
        );
        m3::reply_vmsg!(msg, m3::errors::Code::Success);
        Ok(())
    });

    // This automatically blocks, listens to the gate, and handles connections
    reqhdl.run(&mut srv)?;

    Ok(())
}
