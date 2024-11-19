#![no_std]

use m3::client::EvidenceSession;
use m3::errors::Error;
use m3::io::LogFlags;
use m3::log;

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let ev = EvidenceSession::new("evidence")?;
    let attestation_id = 1;
    let nonce = 42;

    log!(LogFlags::Info, "req quote");
    let quote = ev.quote(attestation_id, nonce)?;
    log!(LogFlags::Info, "received quote: {}", quote);
    Ok(())
}
