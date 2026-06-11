use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::str;

fn main() {
    let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let bind_addr = SocketAddr::new(ip, 0);
    let server_addr = SocketAddr::new(ip, 8080);

    let socket = UdpSocket::bind(bind_addr).expect("Failed to bind client");

    socket
        .set_nonblocking(true)
        .expect("Failed to set non-blocking");

    let max_messages = 5;
    let mut buf = [0; u16::MAX as usize];

    for i in 1..=max_messages {
        let msg = format!("Ping number {}", i);
        println!("Client trying to send: '{}'", msg);
        loop {
            socket.send_to(msg.as_bytes(), server_addr).ok();

            // Check if the server caught it and replied
            if let Ok((amt, _src)) = socket.recv_from(&mut buf) {
                let reply = str::from_utf8(&buf[..amt]).unwrap_or("Invalid UTF-8");
                println!("Client got reply: '{}'", reply);
                break; // Break the loop and move to the next message.
            }

            // give up the CPU
            std::thread::yield_now();
        }
        println!("---");
    }

    println!("Finished exchanging {} messages.", max_messages);
}
