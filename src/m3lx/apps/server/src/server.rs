use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::str;

fn main() {
    let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let bind_addr = SocketAddr::new(ip, 8080);

    let socket = UdpSocket::bind(bind_addr).expect("Failed to bind server");
    println!("Server listening on 127.0.0.1:8080...");

    let mut buf = [0; u16::MAX as usize];

    loop {
        let (amt, src) = socket.recv_from(&mut buf).expect("Failed to receive data");

        let received_msg = str::from_utf8(&buf[..amt]).unwrap_or("Invalid UTF-8");
        println!("Server received: '{}' from {}", received_msg, src);

        let response = format!("Server acknowledgment of: {}", received_msg);

        socket
            .send_to(response.as_bytes(), &src)
            .expect("Failed to send data");
    }
}
