/*
 * Copyright (C) 2024 Nils Asmussen, Barkhausen Institut
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

#![no_std]

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use m3::cell::LazyStaticCell;
use m3::client::Network;
use m3::col::String;
use m3::com::Semaphore;
use m3::errors::{Code, Error};
use m3::io::Read;
use m3::net::{Endpoint, IpAddr, Socket, StreamSocketArgs, TcpSocket};
use m3::test::{DefaultWvTester, WvTester};
use m3::vec::Vec;
use m3::{env, println, vec, wv_assert_eq, wv_assert_ok, wv_require_ok, wv_run_suite, wv_run_test};
use rot::cert::{BinaryPayload, Certificate, ChallengePayload, M3RawCertificate, SignaturePayload};

pub static DST_IP: LazyStaticCell<IpAddr> = LazyStaticCell::default();

fn parse_ip(ip: &str) -> IpAddr {
    ip.parse::<IpAddr>()
        .unwrap_or_else(|_| panic!("{}", m3::format!("Invalid IP address: {}", ip)))
}

fn suite(t: &mut dyn WvTester) {
    wv_run_test!(t, challenge);
}

fn challenge(t: &mut dyn WvTester) {
    const CHALLENGE: &str = "My dummy challenge";

    let net = wv_require_ok!(Network::new("net"));
    let mut socket = wv_require_ok!(TcpSocket::new(StreamSocketArgs::new(net)));

    // wait until server is listening
    wv_assert_ok!(t, Semaphore::attach("net-tcp").unwrap().down());

    // connect and send challenge
    wv_assert_ok!(t, socket.connect(Endpoint::new(DST_IP.get(), 4242)));
    wv_assert_ok!(t, socket.send(CHALLENGE.as_bytes()));

    // read response size
    let mut size_bytes = [0u8; 8];
    wv_assert_ok!(t, socket.recv(&mut size_bytes));
    let size = u64::from_ne_bytes(size_bytes);

    // read response
    let mut buf = vec![0u8; size as usize];
    wv_assert_ok!(t, socket.read_exact(&mut buf));
    let s = String::from_utf8(buf).unwrap();
    println!("Received: {}", s);

    // unserialize certificate
    type RotCRawCert = rot::cert::Certificate<BinaryPayload, M3RawCertificate>;
    let cert: Certificate<ChallengePayload<'_>, RotCRawCert> =
        wv_require_ok!(rot::json::from_str(&s));
    // verify challenge
    wv_assert_eq!(t, cert.payload.challenge, CHALLENGE);

    fn verify_signature<T: SignaturePayload, P>(t: &mut dyn WvTester, cert: &Certificate<T, P>) {
        let verifying_key: VerifyingKey = wv_require_ok!(VerifyingKey::from_bytes(&cert.pub_key));
        wv_assert_ok!(
            t,
            verifying_key.verify(
                cert.payload.as_bytes(),
                &Signature::from_bytes(&cert.signature)
            )
        );
    }

    // verify certificate chain
    let rotc_cert = &cert.parent;
    verify_signature(t, rotc_cert);
    let m3raw_cert = &rotc_cert.parent;
    verify_signature(t, m3raw_cert);
    let blau_cert = &m3raw_cert.parent;
    verify_signature(t, blau_cert);

    // TODO note that we actually would need to verify the payloads as well and check whether the
    // public key in the lowest layer is the expected one (which we noted down during device
    // setup).
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let args: Vec<&str> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: {} <dst-IP>", args[0]);
        return Err(Error::new(Code::InvArgs));
    }

    DST_IP.set(parse_ip(args[1]));

    let mut tester = DefaultWvTester::default();
    wv_run_suite!(tester, suite);
    println!("{}", tester);
    Ok(())
}
