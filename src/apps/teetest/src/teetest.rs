#![no_std]

use m3::client::EvidenceSession;
use m3::errors::Error;
use m3::io::LogFlags;
use m3::log;

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let ev = EvidenceSession::new("evidence")?;
    let app_id = 0;

    let quote = ev.quote(app_id)?;
    log!(LogFlags::Info, "received quote: {}", quote);
    Ok(())
}
