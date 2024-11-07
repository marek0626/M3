use crate::client::ClientSession;
use crate::col::String;
use crate::com::{opcodes, RecvGate, SendGate};
use crate::errors::Error;

// Needed for capability retention
pub struct EvidenceSession {
    _sess: ClientSession,
    sgate: SendGate,
}

impl EvidenceSession {
    pub fn new(name: &str) -> Result<Self, Error> {
        let sess = ClientSession::new(name)?;
        let sgate = sess.connect()?;

        Ok(EvidenceSession { _sess: sess, sgate })
    }

    pub fn quote(&self, app_id: u32) -> Result<String, Error> {
        send_recv_res!(
            self.sgate,
            RecvGate::def(),
            opcodes::Pager::Quote,
            42,
            app_id
        )?
        .pop()
    }
}
