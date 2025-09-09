/*
 * Copyright (C) 2023-2024, Stephan Gerhold <stephan@gerhold.net>
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

#[allow(unused_extern_crates)]
extern crate unimux;

#[allow(unused_extern_crates)]
extern crate lang;

use core::cmp::min;

use base::env::{self, BaseEnv};
use base::mem::PhysAddrRaw;
use base::{cfg, tcu};
use heapsimple::create_heap;
use kecacc::{KecAcc, KecAccState};
use m3core::cap::SelSpace;
use m3core::cell::{LazyReadOnlyCell, LazyStaticRefCell, StaticCell, StaticRefCell};
use m3core::col::{Vec, VecDeque};
use m3core::com::{opcodes, EpMng, GateIStream, LazyGate, MemCap, Perm, RecvGate, EP};
use m3core::crypto::{HashAlgorithm, HashType};
use m3core::errors::{Code, Error};
use m3core::io::LogFlags;
use m3core::kif::{CapRngDesc, CapType};
use m3core::mem::{size_of, GlobOff, MsgBuf, MsgBufRef};
use m3core::serialize::bytes::Bytes;
use m3core::server::{
    server_loop, CapExchange, ClientManager, ExcType, RequestHandler, RequestSession, Server,
    ServerSession, SessId,
};
use m3core::tcu::{EpId, Message, TCU};
use m3core::tiles::Activity;
use m3core::time::{TimeDuration, TimeInstant};
use m3core::{build_vmsg, const_assert, log, reply_vmsg};
use rot::ed25519::Signer;
use rot::{ed25519, Hex, OpaqueKMacKey, Secret};

mod exregs;

rot::generate_entry!();

create_heap!(8 * 1024);

fn check_std_endpoints() {
    let tile_desc = env::boot().tile_desc();
    let mut rbuf_addr = tile_desc.rbuf_std_space().0.as_phys(tile_desc);
    TCU::check_recv_ep(
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
        rbuf_addr,
        cfg::SYSC_RBUF_SIZE,
        false,
    )
    .expect("SYSC_REP not sane");
    rbuf_addr += cfg::SYSC_RBUF_SIZE as PhysAddrRaw;

    TCU::check_recv_ep(
        tcu::FIRST_USER_EP + tcu::UPCALL_REP_OFF,
        rbuf_addr,
        cfg::UPCALL_RBUF_SIZE,
        true,
    )
    .expect("UPCALL_REP not sane");
    rbuf_addr += cfg::UPCALL_RBUF_SIZE as PhysAddrRaw;

    TCU::check_recv_ep(
        tcu::FIRST_USER_EP + tcu::DEF_REP_OFF,
        rbuf_addr,
        cfg::DEF_RBUF_SIZE,
        false,
    )
    .expect("DEF_REP_OFF not sane");

    // now unfreeze all the standard EPs created by the kernel
    if env!("M3_TARGET") == "gem5" || env!("M3_TARGET") == "hw" {
        TCU::unfreeze(tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF).unwrap();
        TCU::unfreeze(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF).unwrap();
        TCU::unfreeze(tcu::FIRST_USER_EP + tcu::UPCALL_REP_OFF).unwrap();
        TCU::unfreeze(tcu::FIRST_USER_EP + tcu::DEF_REP_OFF).unwrap();
    }
}

#[no_mangle]
pub extern "C" fn env_run() {
    let rots_env_src: &BaseEnv =
        unsafe { &*((cfg::ENV_START.as_local() + cfg::ENV_SIZE / 2) as *const _) };
    let rots_env: &mut BaseEnv = unsafe { &mut *(cfg::ENV_START.as_mut_ptr()) };

    // initialize the non-boot fields
    *rots_env = env::BaseEnv {
        boot: rots_env.boot,
        ..Default::default()
    };

    // hard code the arguments here
    rots_env.boot.argc = 1;
    rots_env.boot.argv = (cfg::ENV_START.as_local() + size_of::<BaseEnv>()) as u64;
    rots_env.boot.envp = rots_env_src.boot.envp;
    let argv = rots_env.boot.argv as *mut *const u8;
    unsafe {
        *argv = c"rots".as_ptr();
    }

    // for simplicity and safety, just check for more arguments to disable exreg zeroing
    if rots_env_src.boot.argc > 1 {
        exregs::disable_zeroing();
    }

    // init the remaining relevant fields
    rots_env.first_std_ep = tcu::FIRST_USER_EP as u64;
    rots_env.first_sel = rots_env_src.first_sel;
    rots_env.heap_size = cfg::MOD_HEAP_SIZE as u64;
    rots_env.rmng_sel = rots_env_src.rmng_sel;
    rots_env.act_id = rots_env_src.act_id;

    // check and unfreeze standard EPs configured by the kernel
    check_std_endpoints();

    m3core::env::init();

    m3core::env::run();
}

const MAX_CLIENTS: usize = 4;
const MAX_MSG_SIZE: usize = 256;

const MAX_DERIVED_SECRET_SIZE: usize = 64;

const HASH_SIZE: usize = rot::cert::HASH_ALGO.output_bytes;
const MAX_SIGN_SIZE: usize = HASH_SIZE + MAX_MSG_SIZE;

static CTX: LazyReadOnlyCell<RotsCtx> = LazyReadOnlyCell::default();

//

/// Size of the two SRAMs used as temporary buffer for TCU transfers
/// and the accelerator.
const BUFFER_SIZE: usize = 4 * 1024; // 4 KiB

/// Maximum number of sessions the multiplexer allows.
const MAX_SESSIONS: usize = 32;

/// The default time slice if not specified in the session arguments.
const DEFAULT_TIME_SLICE: TimeDuration = TimeDuration::from_micros(100);

/// Wait until the accelerator is done before triggering a new read/write via
/// the TCU. Enabling this effectively disables the performance benefit of the
/// double buffering approach, which can be used for performance comparison.
const DISABLE_DOUBLE_BUFFER: bool = false;

/// Force saving/loading accelerator state even if the client is already loaded
/// in the accelerator. Mostly useful for benchmarking.
const FORCE_CONTEXT_SWITCH: bool = false;

/// Enables an optimization that checks for new messages while the accelerator
/// is busy. The optimization is most useful when there is often just one client
/// active at the same time and there are short time slices.
const OPTIMIZE_SCHEDULING: bool = true;

// Two memory regions used as temporary buffers (accessed by TCU and accelerator)
// FIXME: This should point to the dedicated SRAMs
static BUF1: StaticRefCell<[u8; BUFFER_SIZE]> = StaticRefCell::new([0u8; BUFFER_SIZE]);
static BUF2: StaticRefCell<[u8; BUFFER_SIZE]> = StaticRefCell::new([0u8; BUFFER_SIZE]);

// Memory region used to save/load states of the accelerator for context switches
const EMPTY_STATE: KecAccState = KecAccState::new();
static STATES: StaticRefCell<[KecAccState; MAX_SESSIONS]> =
    StaticRefCell::new([EMPTY_STATE; MAX_SESSIONS]);

/// Amount of bytes that may be directly returned as part of the TCU reply.
/// Must also fit into [`MsgBuf::borrow_def()`].
const MAX_DIRECT_SIZE: usize = HashAlgorithm::MAX_OUTPUT_BYTES;
const_assert!(MAX_DIRECT_SIZE <= BUFFER_SIZE);

static CURRENT: StaticCell<Option<SessId>> = StaticCell::new(None);
static QUEUE: LazyStaticRefCell<VecDeque<SessId>> = LazyStaticRefCell::default();
static KECACC: KecAcc = KecAcc::new();
//

#[derive(Debug)]
enum HashRequestType {
    /// Read bytes from the MemGate and input them into the hash.
    Input,
    /// Output bytes from the hash and write them into the MemGate.
    Output,
    /// Output bytes from the hash and write them into the MemGate. Pad first.
    OutputPad,
    /// Output a few bytes from the hash and send them directly in the reply.
    OutputDirect,
}

/// A delayed request from a client that will be processed with scheduling.
struct HashRequest {
    ty: HashRequestType,
    msg: &'static Message,
    len: usize,
    off: usize,
}

struct RotsCtx {
    kmac_cdi: Secret<OpaqueKMacKey>,
    signing_key: ed25519::SigningKey,
    rot_cert_cap: MemCap,
    rot_cert_size: GlobOff,
}

struct RoTSession {
    serv: ServerSession,
    rot_sig_cap: Option<MemCap>,
    arg_hash: Hex<[u8; HASH_SIZE]>,
    kmac_cdi: Option<Secret<OpaqueKMacKey>>,
    secret_cap: Option<LazyGate<MemCap>>,
    mem: Option<EP>,
    algo: Option<&'static HashAlgorithm>,
    state_saved: bool,
    req: Option<HashRequest>,
    time_slice: TimeDuration,
    remaining_time: i64, // in nanoseconds
    output_bytes: usize,
}

impl RequestSession for RoTSession {
    /// ROTS provides two things: Root of Trust and hash acceleration
    /// functionality. ROTS can provide either/or without panicking.
    fn new(serv: ServerSession, arg: &str) -> Result<Self, Error> {
        // we need at least some seed to produce valid hashes
        if arg.is_empty() {
            return Err(Error::new(Code::InvArgs));
        }

        fn have_ctx(arg: &str) -> (Option<MemCap>, Option<Secret<OpaqueKMacKey>>) {
            let ctx = CTX.get();
            let rot_sig_cap = ctx
                .rot_cert_cap
                .derive(0, ctx.rot_cert_size, Perm::R)
                .expect("Couldn't derive");
            let mut kmac_cdi = Secret::new_zeroed();
            // TODO this uses KECACC internally which forces us to save/restore the state
            // before/after if required. maybe this should be changed to have a wrapper that
            // performs the save/restore instead of doing it manually at all places.
            rot::derive_cdi(&ctx.kmac_cdi, arg.as_bytes(), &mut kmac_cdi);
            (Some(rot_sig_cap), Some(kmac_cdi))
        }

        let sid = serv.id();
        log!(LogFlags::RoTReqs, "[{}] rot::new()", sid);
        assert!(serv.id() < MAX_SESSIONS);

        // Use the time slice specified in the arguments or fall back to the default one
        // TODO expect arguments in a proper format
        let time_slice = if !arg.is_empty() {
            match arg.parse::<u64>() {
                Ok(time) => TimeDuration::from_nanos(time),
                Err(_) => DEFAULT_TIME_SLICE,
            }
        }
        else {
            DEFAULT_TIME_SLICE
        };

        // Swap any context here
        let cur_id_opt = CURRENT.get();
        let mut state = STATES.borrow_mut();
        if let Some(cur_id) = cur_id_opt {
            KECACC.start_save(&mut state[cur_id]);
        }

        // Only if we detected and configured a valid Root of Trust context at activity startup, do this part
        let ctx_data = match CTX.is_some() {
            true => have_ctx(arg),
            false => (None, None),
        };

        let mut sess = Self {
            serv,
            rot_sig_cap: ctx_data.0,
            arg_hash: Hex::new_zeroed(),
            kmac_cdi: ctx_data.1,
            secret_cap: None,
            mem: None,
            algo: None,
            state_saved: false,
            req: None,
            time_slice,
            remaining_time: 0,
            output_bytes: 0,
        };

        rot::hash(rot::cert::HASH_TYPE, arg.as_bytes(), &mut sess.arg_hash[..]);

        // And restore it. Not sure that I need to do this, really
        if let Some(cur_id) = cur_id_opt {
            KECACC.start_load(&state[cur_id]);
        }

        log!(
            LogFlags::RoTReqs,
            "Hash for client '{}': {}",
            arg,
            sess.arg_hash
        );
        Ok(sess)
    }

    fn close(&mut self, _cli: &mut ClientManager<Self>, sid: SessId, _sub_ids: &mut Vec<SessId>)
    where
        Self: Sized,
    {
        log!(LogFlags::HMuxReqs, "[{}] hash::close()", sid);

        QUEUE.borrow_mut().retain(|&n| n != sid);
        if CURRENT.get() == Some(sid) {
            CURRENT.set(None);
        }

        // Revoke EP capability that was delegated for memory accesses
        if let Some(ep) = self.mem.take() {
            EpMng::get().release(ep, true);
        }
    }
}

/// Run func repeatedly with the two buffers swapped,
/// until an error occurs or func returns Ok(false).
fn loop_double_buffer<F>(mut func: F) -> Result<(), Error>
where
    F: FnMut(&mut [u8], &mut [u8]) -> Result<bool, Error>,
{
    let buf1 = &mut BUF1.borrow_mut()[..];
    let buf2 = &mut BUF2.borrow_mut()[..];
    while func(buf1, buf2)? && func(buf2, buf1)? {}
    Ok(())
}

/// Handles time accounting when working on a request by a client.
struct HashMuxTimer {
    /// The time slice configured for the client, copied from the session.
    slice: TimeDuration,
    /// The time in TCU nano seconds when the clients time expires.
    end: TimeInstant,
}

impl HashMuxTimer {
    fn start(sess: &RoTSession) -> Self {
        HashMuxTimer {
            slice: sess.time_slice,
            // wrapping_add handles unsigned + signed addition here
            end: TimeInstant::from_nanos(
                TimeInstant::now()
                    .as_nanos()
                    .wrapping_add(sess.remaining_time as u64),
            ),
        }
    }

    /// Wait until the accelerator is really done and calculate remaining time.
    fn remaining_time(&self, mut t: TimeInstant) -> i64 {
        if KECACC.is_busy() {
            KECACC.poll_complete();
            t = TimeInstant::now();
        }

        // This might underflow (end < now()), resulting in a negative number after the cast
        self.end.as_nanos().wrapping_sub(t.as_nanos()) as i64
    }

    /// Check if the client has run out of time. If yes, return how much additional
    /// time the client has used (= a negative amount of remaining time).
    ///
    /// An additional optimization done here is to check if there are actually
    /// any other clients that have work ready. If no other client is ready,
    /// the current one can just keep working without interrupting the
    /// double buffering. This is slightly faster (although not that much
    /// with large enough time slices).
    ///
    /// This optimization would mainly allow reducing latency if prioritization
    /// is needed at some point. The time when the accelerator is busy could
    /// be used to check pending messages more frequently, without adjusting
    /// the time slice of low-priority clients.
    fn try_continue(&mut self, req_rgate: &RecvGate, srv_rgate: &RecvGate) -> Result<(), i64> {
        let t = TimeInstant::now();
        if t >= self.end {
            // Time expired, look for other clients to work on
            if !OPTIMIZE_SCHEDULING
                || !QUEUE.borrow().is_empty()
                || req_rgate.has_msgs()
                || srv_rgate.has_msgs()
            {
                return Err(self.remaining_time(t));
            }

            // No one else has work ready so just keep going
            self.end += self.slice;
        }
        Ok(())
    }

    /// Calculate the remaining time the client has if work completes early
    /// (because it is done or because of an error).
    fn finish(self) -> i64 {
        self.remaining_time(TimeInstant::now())
    }
}

impl HashRequest {
    /// Try to take up to one full buffer from the request and return the
    /// size of it (typically the buffer size unless there is not enough left).
    fn take_buffer_size(&mut self) -> usize {
        let n = min(self.len, BUFFER_SIZE);
        self.len -= n;
        n
    }

    /// Complete work on a buffer size obtained by [take_buffer_size()].
    fn complete_buffer(&mut self, n: usize) {
        self.off += n;
    }

    /// Reply with code to the original request.
    fn reply(self, code: Code, req_rgate: &RecvGate) {
        let mut msg = MsgBuf::borrow_def();
        build_vmsg!(&mut msg, code);
        self.reply_msg(msg, req_rgate)
    }

    /// Reply with the specified message to the original request.
    fn reply_msg(self, msg: MsgBufRef, req_rgate: &RecvGate) {
        req_rgate
            .reply(&msg, self.msg)
            .or_else(|_| req_rgate.ack_msg(self.msg))
            .ok();
    }
}

impl RoTSession {
    fn id(&self) -> SessId {
        self.serv.id()
    }

    fn sid(&self) -> SessId {
        self.serv.id()
    }

    fn get_rot_certificate(
        cli: &mut ClientManager<Self>,
        _crt: usize,
        sid: SessId,
        xchg: &mut CapExchange<'_>,
    ) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::get_rot_certificate()", sid);
        if !CTX.is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        let ctx = CTX.get();
        let sess = cli.get(sid).ok_or_else(|| Error::new(Code::InvArgs))?;
        xchg.out_caps(CapRngDesc::new_single(
            CapType::Object,
            sess.rot_sig_cap.as_ref().unwrap().sel(),
        ));
        xchg.out_args().push(0);
        xchg.out_args().push(ctx.rot_cert_size);
        Ok(())
    }

    fn set_secret_mem(
        cli: &mut ClientManager<Self>,
        _crt: usize,
        sid: SessId,
        xchg: &mut CapExchange<'_>,
    ) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::get_secret_mem()", sid);
        if !CTX.is_some() {
            return Err(Error::new(Code::NotSup));
        }

        let sess = cli.get_mut(sid).ok_or_else(|| Error::new(Code::InvArgs))?;
        if sess.secret_cap.is_some() {
            return Err(Error::new(Code::Exists));
        }

        let sel = SelSpace::get().alloc_sel();
        sess.secret_cap = Some(LazyGate::new((sel, false)));
        xchg.out_caps(CapRngDesc::new_single(CapType::Object, sel));
        Ok(())
    }

    fn get_hash(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::get_hash()", self.sid());
        if !CTX.is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        reply_vmsg!(is, Code::Success, Bytes::new(&self.arg_hash[..]))
    }

    fn get_cdi(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::get_cdi()", self.sid());
        if !CTX.is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        if let Some(kmac_cdi) = &self.kmac_cdi {
            let vec = kmac_cdi.get().to_vec();
            reply_vmsg!(is, Code::Success, vec)
        }
        else {
            reply_vmsg!(is, Code::InvArgs)
        }
    }

    fn derive_secret(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::derive_secret()", self.sid());
        if !CTX.is_some() {
            return Err(Error::new(Code::InvArgs));
        }
        let custom: &str = is.pop()?;
        let size: usize = is.pop()?;
        if size > MAX_DERIVED_SECRET_SIZE {
            return Err(Error::new(Code::InvArgs));
        }

        let Some(kmac_cdi) = &self.kmac_cdi
        else {
            return Err(Error::new(Code::InvArgs));
        };
        let Some(secret_cap) = &mut self.secret_cap
        else {
            return Err(Error::new(Code::InvArgs));
        };

        // save current state
        let cur_id_opt = CURRENT.get();
        let mut state = STATES.borrow_mut();
        if let Some(cur_id) = cur_id_opt {
            KECACC.start_save(&mut state[cur_id]);
        }

        let mut temp = [0u8; MAX_DERIVED_SECRET_SIZE];
        rot::derive_key(kmac_cdi, custom, &[], &mut temp[..size]);

        // and restore the state again
        if let Some(cur_id) = cur_id_opt {
            KECACC.start_load(&state[cur_id]);
        }

        secret_cap.get().unwrap().write_obj(&temp, 0)?;

        reply_vmsg!(is, Code::Success)
    }

    fn certify(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::RoTReqs, "[{}] rot::certify()", self.sid());

        // Sign requested payload with identity prepended
        let mut buf = [0u8; MAX_SIGN_SIZE];
        buf[0..HASH_SIZE].copy_from_slice(&self.arg_hash[..]);
        let bytes: &[u8] = is.pop()?;
        let end = HASH_SIZE + bytes.len();
        buf[HASH_SIZE..end].copy_from_slice(bytes);

        log!(LogFlags::RoTReqs, "Signing: {}", Hex(&buf[..end]));
        let ctx = CTX.get();
        let signature = ctx.signing_key.sign(&buf[..end]);
        log!(LogFlags::RoTDbg, "Signed: {:x}", signature);

        reply_vmsg!(is, Code::Success, Bytes::new(&signature.to_bytes()[..]))
    }

    fn try_with_ep<F>(&self, mut func: F) -> Result<(), Error>
    where
        F: FnMut(EpId) -> Result<(), Error>,
    {
        let ep = self.mem.as_ref().unwrap().id();
        match func(ep) {
            Err(e) if e.code() == Code::NoMEP => {
                if TCU::is_frozen(ep) {
                    TCU::unfreeze(ep)?;
                }
                func(ep)
            },
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    }

    /// Work on an input request by reading memory from the MemGate and
    /// letting the accelerator absorb it.
    fn work_input(
        &mut self,
        mut req: HashRequest,
        req_rgate: &RecvGate,
        srv_rgate: &RecvGate,
    ) -> bool {
        let mut timer = HashMuxTimer::start(self);

        let res = loop_double_buffer(|buf, _| {
            let n = req.take_buffer_size();
            if n == 0 {
                return Err(Error::new(Code::Success)); // Done
            }

            self.try_with_ep(|ep| TCU::read(ep, buf.as_mut_ptr(), n, req.off as u64))?;

            KECACC.start_absorb(&buf[..n]);
            if DISABLE_DOUBLE_BUFFER {
                KECACC.poll_complete();
            }
            req.complete_buffer(n);

            if let Err(remaining_time) = timer.try_continue(req_rgate, srv_rgate) {
                self.remaining_time = remaining_time;
                return Ok(false);
            }
            Ok(true)
        });
        self.handle_result(res, req, timer, req_rgate)
    }

    /// Work on an output request by letting the accelerator squeeze it
    /// and then writing it to the MemGate.
    fn work_output(
        &mut self,
        mut req: HashRequest,
        req_rgate: &RecvGate,
        srv_rgate: &RecvGate,
    ) -> bool {
        let mut timer = HashMuxTimer::start(self);

        // Apply padding once for the request if needed
        if matches!(req.ty, HashRequestType::OutputPad) {
            KECACC.start_pad();
            req.ty = HashRequestType::Output;
        }

        // Output is a bit more complicated than input because the MemGate is
        // always used synchronously (poll until done), while the accelerator
        // runs asynchronously (start, do other work, poll if not done yet).
        // Since the accelerator runs first for output, it needs to finish up
        // one buffer first before the double buffering can be started.
        // This also means that the last squeezed buffer always need to be
        // written back to the MemGate before returning from this function.

        let mut ln = req.take_buffer_size();
        KECACC.start_squeeze(&mut BUF1.borrow_mut()[..ln]);

        let res = loop_double_buffer(|lbuf, buf| {
            let n = req.take_buffer_size();
            if n == 0 {
                // Still need to write back the last buffer
                KECACC.poll_complete();
                self.try_with_ep(|ep| TCU::write(ep, lbuf.as_ptr(), ln, req.off as u64))?;
                return Err(Error::new(Code::Success)); // Done
            }

            // Start squeezing *new* buffer and write back the *last* buffer
            KECACC.start_squeeze(&mut buf[..n]);
            if DISABLE_DOUBLE_BUFFER {
                KECACC.poll_complete();
            }

            self.try_with_ep(|ep| TCU::write(ep, lbuf.as_ptr(), ln, req.off as u64))?;
            req.complete_buffer(ln);

            if let Err(remaining_time) = timer.try_continue(req_rgate, srv_rgate) {
                // Still need to write back the last buffer - this might take a bit
                // so measure the time and subtract it from the remaining time.
                let t = TimeInstant::now();
                self.try_with_ep(|ep| TCU::write(ep, buf.as_ptr(), n, req.off as u64))?;
                req.complete_buffer(n);
                self.remaining_time = remaining_time - (TimeInstant::now() - t).as_nanos() as i64;
                return Ok(false);
            }

            ln = n;
            Ok(true)
        });
        self.handle_result(res, req, timer, req_rgate)
    }

    fn handle_result(
        &mut self,
        res: Result<(), Error>,
        req: HashRequest,
        timer: HashMuxTimer,
        req_rgate: &RecvGate,
    ) -> bool {
        match res {
            Ok(_) => {
                // More work needed, restore request
                self.req = Some(req);

                log!(
                    LogFlags::HMuxDbg,
                    "[{}] hash::work() pause, remaining time {}",
                    self.serv.id(),
                    self.remaining_time,
                );
                false // not done
            },
            Err(e) => {
                req.reply(e.code(), req_rgate);
                self.remaining_time = timer.finish();

                if e.code() == Code::Success {
                    log!(
                        LogFlags::HMuxInOut,
                        "[{}] hash::work() done, remaining time {}",
                        self.serv.id(),
                        self.remaining_time,
                    )
                }
                else {
                    log!(
                        LogFlags::Error,
                        "[{}] hash::work() failed with {:?}",
                        self.serv.id(),
                        e.code(),
                    );
                }
                true // done
            },
        }
    }

    /// Work on a direct output request by letting the accelerator squeeze
    /// a few bytes and then sending them directly as reply.
    fn work_output_direct(&mut self, req: HashRequest, req_rgate: &RecvGate) -> bool {
        let timer = HashMuxTimer::start(self);
        let buf = &mut BUF1.borrow_mut()[..req.len];
        let mut msg = MsgBuf::borrow_def();

        KECACC.start_pad();
        KECACC.start_squeeze(buf);
        KECACC.poll_complete_barrier();
        msg.set_from_slice(buf);

        req.reply_msg(msg, req_rgate);
        self.remaining_time = timer.finish();
        log!(
            LogFlags::HMuxInOut,
            "[{}] hash::work() done, remaining time {}",
            self.serv.id(),
            self.remaining_time,
        );
        true // done
    }

    fn work(&mut self, req_rgate: &RecvGate, srv_rgate: &RecvGate) -> bool {
        // Fill up time of client. Subtract time from time slice if client took too long last time
        if self.remaining_time < 0 {
            self.remaining_time += self.time_slice.as_nanos() as i64;
        }
        else {
            self.remaining_time = self.time_slice.as_nanos() as i64;
        }
        let req = self.req.take().unwrap();

        log!(
            LogFlags::HMuxDbg,
            "[{}] hash::work() {:?} start len {} off {} remaining time {} queue {:?}",
            self.serv.id(),
            req.ty,
            req.len,
            req.off,
            self.remaining_time,
            QUEUE.borrow(),
        );

        match req.ty {
            HashRequestType::Input => self.work_input(req, req_rgate, srv_rgate),
            HashRequestType::Output | HashRequestType::OutputPad => {
                self.work_output(req, req_rgate, srv_rgate)
            },
            HashRequestType::OutputDirect => self.work_output_direct(req, req_rgate),
        }
    }

    fn features(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        let mut features: m3core::client::Features = m3core::client::Features::HashMux;
        if CTX.is_some() {
            features |= m3core::client::Features::RoT;
        }
        reply_vmsg!(is, Code::Success, features)
    }

    fn reset(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        let ty: HashType = is.pop()?;
        let algo = HashAlgorithm::from_type(ty).ok_or_else(|| Error::new(Code::InvArgs))?;

        log!(
            LogFlags::HMuxInOut,
            "[{}] hash::reset() algo {}",
            self.serv.id(),
            algo
        );

        if !KECACC.supports_algo(algo.ty) {
            log!(
                LogFlags::Error,
                "[{}] Algorithm {} not supported",
                self.serv.id(),
                algo
            );
            return Err(Error::new(Code::NotSup));
        }

        assert!(self.req.is_none());
        self.algo = Some(algo);
        self.state_saved = false;
        self.output_bytes = 0;

        if is.reply_error(Code::Success).is_ok() {
            // Clear current session if necessary to force re-initialization
            if CURRENT.get() == Some(is.label() as SessId) {
                CURRENT.set(None);
            }
        }

        Ok(())
    }

    fn get_mem(
        cli: &mut ClientManager<Self>,
        _crt: usize,
        sid: SessId,
        xchg: &mut CapExchange<'_>,
    ) -> Result<(), Error> {
        log!(LogFlags::HMuxReqs, "[{}] hash::obtain()", sid);
        let hash = cli.get_mut(sid).ok_or_else(|| Error::new(Code::InvArgs))?;

        if hash.mem.is_some() {
            return Err(Error::new(Code::Exists));
        }

        let ep = EpMng::get().acquire(0, false)?;
        let ep_sel = ep.sel();
        hash.mem = Some(ep);
        xchg.out_caps(CapRngDesc::new_single(CapType::Object, ep_sel));

        Ok(())
    }

    /// Queue a new request for the client and mark client as ready and perhaps
    /// even waiting if it has remaining time.
    fn queue_request(&mut self, req: HashRequest, req_rgate: &RecvGate) -> Result<(), Error> {
        assert!(self.req.is_none());

        if req.len > 0 {
            self.req = Some(req);
            QUEUE.borrow_mut().push_back(self.id());
        }
        else {
            // This is weird but not strictly wrong, just return immediately
            req.reply(Code::Success, req_rgate);
        }
        Ok(())
    }

    fn input(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::HMuxInOut, "[{}] hash::input()", self.serv.id());

        // Disallow input after output for now since this is not part of the SHA-3 specification.
        // However, there is a separate paper about the "Duplex" construction:
        // https://keccak.team/files/SpongeDuplex.pdf, so this could be changed if needed.
        if self.output_bytes > 0 {
            return Err(Error::new(Code::InvState));
        }

        self.queue_request(
            HashRequest {
                ty: HashRequestType::Input,
                off: is.pop()?,
                len: is.pop()?,
                msg: is.take_msg(),
            },
            is.rgate(),
        )
    }

    fn output(&mut self, is: &mut GateIStream<'_>) -> Result<(), Error> {
        log!(LogFlags::HMuxInOut, "[{}] hash::output()", self.serv.id());

        let algo = self.algo.ok_or_else(|| Error::new(Code::InvState))?;

        let req = if is.size() > size_of::<u64>() {
            HashRequest {
                ty: if self.output_bytes == 0 {
                    // On first output padding still needs to be applied to the input
                    HashRequestType::OutputPad
                }
                else {
                    HashRequestType::Output
                },
                off: is.pop()?,
                len: is.pop()?,
                msg: is.take_msg(),
            }
        }
        else {
            // No parameters, return the fixed size hash directly in the TCU reply
            if algo.output_bytes > MAX_DIRECT_SIZE {
                log!(
                    LogFlags::Error,
                    "[{}] hash::output() cannot use direct output for {}",
                    self.serv.id(),
                    algo.name,
                );
                return Err(Error::new(Code::InvArgs));
            }

            HashRequest {
                ty: HashRequestType::OutputDirect,
                len: algo.output_bytes,
                msg: is.take_msg(),
                off: 0,
            }
        };

        // Verify that the client does not consume more bytes than supported for the hash algorithm
        self.output_bytes = self.output_bytes.saturating_add(req.len);
        if self.output_bytes > algo.output_bytes {
            log!(
                LogFlags::Error,
                "[{}] hash::output() attempting to output {} bytes while only {} are supported for {}",
                self.serv.id(),
                self.output_bytes,
                algo.output_bytes,
                algo.name
            );

            req.reply(Code::InvArgs, is.rgate());
            return Ok(());
        }

        self.queue_request(req, is.rgate())
    }
}

/// Receives requests from kernel and clients through `RecvGate`s.
struct HashMuxReceiver {
    server: Server,
    reqhdl: RequestHandler<RoTSession, opcodes::RoT>,
}

impl HashMuxReceiver {
    fn work(&mut self, sid: SessId) -> bool {
        let Self { server, reqhdl } = self;
        let serv_rgate = server.rgate();
        reqhdl
            .clients_mut()
            .with(sid, |sess, rgate| Ok(sess.work(rgate, serv_rgate)))
            .unwrap()
    }

    fn handle_messages(&mut self) -> Result<(), Error> {
        // NOTE: Currently this only fetches a single message, perhaps handle()
        // should return if a message was handled so this could be put in a loop.
        self.server.fetch_and_handle(&mut self.reqhdl)?;
        self.reqhdl.fetch_and_handle_msg();
        Ok(())
    }

    /// Switch the current client in the accelerator if necessary
    /// by saving/loading the state.
    fn switch(&mut self, to: SessId) {
        let mut state = STATES.borrow_mut();

        if let Some(cur) = CURRENT.get() {
            if !FORCE_CONTEXT_SWITCH && cur == to {
                // Already the current client
                return;
            }

            // Save state of current client
            KECACC.start_save(&mut state[cur]);
            self.reqhdl
                .clients_mut()
                .get_mut(cur as SessId)
                .unwrap()
                .state_saved = true;
        }
        CURRENT.set(Some(to));

        // Restore state of new client or initialize it if necessary
        let sess = self.reqhdl.clients_mut().get(to).unwrap();
        if sess.state_saved {
            KECACC.start_load(&state[to]);
        }
        else {
            KECACC.start_init(sess.algo.unwrap().ty);
        }
    }
}

fn init_rot() -> Result<(MemCap, u64), Error> {
    log!(LogFlags::RoTDbg, "cert load");
    let rot_cert_cap = MemCap::new_bind_bootmod("rot-certificate.json")?;

    log!(LogFlags::RoTDbg, "cert rgn");
    let rot_cert_size = match rot_cert_cap.region() {
        Ok(rgn) => rgn.1,
        Err(e) => return Err(e),
    };
    Ok((rot_cert_cap, rot_cert_size))
}

#[no_mangle]
pub fn init_exregs() {
    unimux::reg_exreg_handler(exregs::add, exregs::rem);
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    log!(LogFlags::RoTBoot, "Hello World!");

    let _eps = {
        let ctx = unsafe { rot::RotsLayerCtx::take() };

        // ensure that the occupied EPs are not reused by the kernel
        let mut eps = Vec::new();
        let (mut ep_start, ep_count) = ctx.data.occupied_eps;
        let ep_end = ep_start + ep_count as tcu::EpId;
        while ep_start < ep_end {
            // to avoid many syscalls we specify replies here, which is of course not true, but
            // currently only leads to 1 + <replies> EPs being allocated.
            let amount = ep_count.min(cfg::MAX_RB_SIZE);
            eps.push(EpMng::acquire_for(
                Activity::own().sel(),
                ep_start,
                amount - 1,
                false,
            ));
            ep_start += amount as tcu::EpId;
        }

        init_rot()
            .map(|res_tuple| {
                log!(LogFlags::RoTDbg, "inited rot successfully");
                let (rot_cert_cap, rot_cert_size) = res_tuple;
                CTX.set(RotsCtx {
                    kmac_cdi: ctx.data.kmac_cdi,
                    signing_key: ed25519::SigningKey::from_bytes(
                        &ctx.data.derived_private_key.secret,
                    ),
                    rot_cert_cap,
                    rot_cert_size,
                });
                log!(LogFlags::RoTDbg, "Derived own {:?}", CTX.get().signing_key);
                Ok::<(), Error>(())
            })
            .map_err(|_| {
                log!(LogFlags::Info, "Disabling RoT functionality.");
                Ok::<(), Error>(())
            })
            .ok();

        eps
    };

    let mut hdl = RequestHandler::new_with(MAX_CLIENTS, MAX_MSG_SIZE, 1)
        .expect("Unable to create request handler");
    let srv = Server::new("rot", &mut hdl).expect("Unable to create service 'rot'");

    use opcodes::RoT;
    hdl.reg_cap_handler(
        RoT::GetRotCertificate,
        ExcType::Obt(1),
        RoTSession::get_rot_certificate,
    );
    hdl.reg_cap_handler(
        RoT::SetSecretMem,
        ExcType::Del(1),
        RoTSession::set_secret_mem,
    );
    hdl.reg_cap_handler(RoT::GetMem, ExcType::Obt(1), RoTSession::get_mem);
    hdl.reg_msg_handler(RoT::GetHash, RoTSession::get_hash);
    hdl.reg_msg_handler(RoT::GetCdi, RoTSession::get_cdi);
    hdl.reg_msg_handler(RoT::DeriveSecret, RoTSession::derive_secret);
    hdl.reg_msg_handler(RoT::Certify, RoTSession::certify);
    hdl.reg_msg_handler(RoT::Reset, RoTSession::reset);
    hdl.reg_msg_handler(RoT::Input, RoTSession::input);
    hdl.reg_msg_handler(RoT::Output, RoTSession::output);
    hdl.reg_msg_handler(RoT::Features, RoTSession::features);

    // Hash Acceleration functionality

    let mut recv = HashMuxReceiver {
        server: srv,
        reqhdl: hdl,
    };

    QUEUE.set(VecDeque::with_capacity(MAX_SESSIONS));

    server_loop(|| {
        // explicitly check for sidecalls on hw23, as we don't get interrupts
        #[cfg(any(M3_TARGET = "hw23", M3_TARGET = "hw"))]
        unimux::check_sidecalls();

        recv.handle_messages()?;

        // The QUEUE is mutably borrowed to pop a session and again later while
        // working on a request. When this is placed directly into the while let
        // Rust only drops the borrow after the loop iteration for some reason.
        // Placing this in a separate function/closure seems to convince it to
        // drop it again immediately...
        let pop_queue = || QUEUE.borrow_mut().pop_front();

        // Keep working until no more work is available
        while let Some(sid) = pop_queue() {
            recv.switch(sid);

            let done = recv.work(sid);

            recv.handle_messages()?;
            if !done {
                QUEUE.borrow_mut().push_back(sid);
            }
        }
        Ok(())
    })
    .ok();

    // we cannot exit normally on hw23 as tmcalls are not supported
    #[cfg(any(M3_TARGET = "hw23", M3_TARGET = "hw"))]
    unimux::exit(Code::Success as u32);

    #[cfg(not(any(M3_TARGET = "hw23", M3_TARGET = "hw")))]
    Ok(())
}
