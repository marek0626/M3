use base::cell::{Ref, StaticCell, StaticRefCell};
use base::cfg;
use base::col::Vec;
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif;
use base::log;
use base::mem::{MsgBuf, VirtAddr, VirtAddrRaw};
use base::serialize::{Deserialize, M3Deserializer};
use base::tcu;

type SidecallHandler = fn(&'static tcu::Message) -> Result<(u64, u64), Error>;
static SIDECALL_HANDLERS: StaticRefCell<Vec<(kif::tilemux::Sidecalls, SidecallHandler)>> =
    StaticRefCell::new(Vec::<(kif::tilemux::Sidecalls, SidecallHandler)>::new());

static RBUF_MUX_ADDR: StaticCell<VirtAddr> = StaticCell::new(VirtAddr::new(0));

pub fn side_rbuf_addr() -> VirtAddr {
    RBUF_MUX_ADDR.get() + cfg::KPEX_RBUF_SIZE as VirtAddrRaw
}

pub(crate) fn init(addr: VirtAddr) {
    RBUF_MUX_ADDR.set(addr);
}

pub fn find_handler(op: kif::tilemux::Sidecalls) -> Option<Ref<'static, SidecallHandler>> {
    find(op, SIDECALL_HANDLERS.borrow())
}

pub(crate) fn find<K, V>(key: K, vec: Ref<'_, Vec<(K, V)>>) -> Option<Ref<'_, V>>
where
    K: core::cmp::PartialEq,
{
    Ref::filter_map(vec, |ref_vec| {
        for (t0, t1) in ref_vec {
            if *t0 == key {
                return Some(t1);
            }
        }
        None
    })
    .ok()
}

pub fn register_sidecall_handler(
    id: kif::tilemux::Sidecalls,
    handler: SidecallHandler,
) -> Result<(), Error> {
    if find(id, SIDECALL_HANDLERS.borrow()).is_some() {
        return Err(Error::new(Code::Exists));
    }
    else {
        SIDECALL_HANDLERS.borrow_mut().push((id, handler));
    }
    Ok(())
}

pub fn reply_msg(msg: &'static tcu::Message, reply: &MsgBuf) {
    let msg_off = tcu::TCU::msg_to_offset(side_rbuf_addr(), msg);
    tcu::TCU::reply(tcu::TMSIDE_REP, reply, msg_off).unwrap();
}

pub fn get_request<'de, R: Deserialize<'de>>(msg: &'static tcu::Message) -> Result<R, Error> {
    let mut de = M3Deserializer::new(msg.as_words());
    de.skip(1);
    de.pop().map_err(|e| e.into())
}
