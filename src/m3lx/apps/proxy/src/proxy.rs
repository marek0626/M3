extern crate m3core as m3;

use m3::com::{RecvGate, recv_msg};
use m3::errors::Code;
use m3::{reply_vmsg, wv_require_ok};

fn main() -> Result<(), std::io::Error> {
    // 1. Initialize the M3 environment so the app can talk to the TCU
    m3::env::init();
    
    println!("======================================");
    println!(" Proxy VFS Server Booting Up! ");
    println!("======================================");

    // 2. Open the receiving gate. 
    // The M3 root dispatcher binds the XML <serv name="proxy_net"> to this gate.
    let rgate = wv_require_ok!(RecvGate::new_named("proxy"));
    
    println!("Proxy successfully registered 'proxy_net' and is listening...");

    // 3. The Interception Loop
    loop {
        // This blocks the proxy until the client sends a network request
        let mut msg = wv_require_ok!(recv_msg(&rgate));
        
        // At this point, the native app has called something like socket() or connect()
        println!(">>> PROXY INTERCEPT: Received a network request from the client! <<<");

        // 4. The Dummy Reply
        // We must reply, otherwise the native app will freeze waiting for the OS to respond.
        // Sending Code::Success tricks the native app into thinking the network call worked.
        wv_require_ok!(reply_vmsg!(msg, Code::Success));
    }
}
