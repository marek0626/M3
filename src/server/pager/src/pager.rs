/*
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
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

mod addrspace;
mod dataspace;
mod mapper;
mod physmem;
mod regions;

use core::fmt;
use core::ops::{Deref, DerefMut};

use m3::boxed::Box;
use m3::cell::LazyStaticRefCell;
use m3::client::{ClientSession, Pager, RoTSession, M3FS};
use m3::col::{String, ToString, Vec};
use m3::com::{opcodes, GateIStream, MemCap, RecvGate, SGateArgs, SendCap};
use m3::crypto::HashAlgorithm;
use m3::errors::{Code, Error};
use m3::format;
use m3::io::LogFlags;
use m3::kif::syscalls::MuxType;
use m3::log;
use m3::mem::VirtAddr;
use m3::reply_vmsg;
use m3::server::{ExcType, RequestHandler, RequestSession, Server, ServerSession};
use m3::tcu::Label;
use m3::tiles::{Activity, ActivityArgs, ChildActivity};
use m3::util::math;
use m3::vfs::{self, File, SeekMode};

use addrspace::AddrSpace;

use resmng::childs::{self, Child, ChildManager, OwnChild};
use resmng::resources::{tiles, Resources};
use resmng::sendqueue;
use resmng::subsys;
use resmng::{config, rerrno};
use resmng::{requests, rerror};

use hex::Hex;

static REQHDL: LazyStaticRefCell<RequestHandler<AddrSpace, opcodes::Pager>> =
    LazyStaticRefCell::default();
static EVREQHDL: LazyStaticRefCell<RequestHandler<EvidenceSession, opcodes::Pager>> =
    LazyStaticRefCell::default();
static ROT: LazyStaticRefCell<RoTSession> = LazyStaticRefCell::default();

#[derive(Default)]
struct PagedChildStarter {
    mounts: Vec<(String, String)>,
}

impl PagedChildStarter {
    fn get_mount(&mut self, name: &str) -> anyhow::Result<String> {
        for (n, mpath) in self.mounts.iter() {
            if n == name {
                return Ok(mpath.clone());
            }
        }

        let id = self.mounts.len();
        let fs = M3FS::new(id, name)
            .map_err(|e| rerror(e).context(format!("open m3fs session {}", name)))?;
        let our_path = format!("/child-mount-{}", name);
        Activity::own()
            .mounts()
            .add(&our_path, fs)
            .map_err(|e| rerror(e).context("add child mount"))?;
        self.mounts.push((name.to_string(), our_path.to_string()));
        Ok(our_path)
    }
}

// Extremely simple Karp-Rabin type hash.
pub fn get_hash(file: &mut dyn File, file_size: usize) -> anyhow::Result<String> {
    let binder = ROT.borrow();
    let rot = binder.deref();
    file.hash_input(rot, file_size)
        .map_err(|e| rerror(e).context("hash file"))?;
    let mut buf = [0u8; HashAlgorithm::SHA3_256.output_bytes];
    rot.finish(&mut buf)
        .map_err(|e| rerror(e).context("finish hash"))?;
    let hex = Hex(&buf[..]);
    log!(LogFlags::Debug, "App hash: {}", hex);
    file.seek(0, SeekMode::Set)
        .map_err(|e| rerror(e).context("seek file for hashing"))?; // NMG Needed when we use hash_input? Probably not.
    Ok(format!("{}", hex))
}

impl subsys::ChildStarter for PagedChildStarter {
    fn start_async(
        &mut self,
        reqs: &requests::Requests,
        res: &mut Resources,
        child: &mut OwnChild,
    ) -> anyhow::Result<()> {
        // send gate for resmng
        let resmng_scap = SendCap::new_with(
            SGateArgs::new(reqs.recv_gate())
                .credits(1)
                .label(Label::from(child.id())),
        )
        .map_err(|e| rerror(e).context("child sendgate"))?;

        // create pager session for child (creator=0 here because we create all sessions ourself)
        let (child_sess, child_sgate, pager_sgate, child_sid) = {
            let mut hdl = REQHDL.borrow_mut();
            let cli = hdl.clients_mut();
            let (crd, nsid) = cli
                .add_connected(0, |_hdl, serv, _sgate| Ok(AddrSpace::new(serv, None, None)))
                .map_err(|e| rerror(e).context("add client connection"))?;
            let pf_sgate = cli
                .add_connection_to(nsid)
                .map_err(|e| rerror(e).context("add client connection for PFs"))?;
            (
                ClientSession::new_bind(crd.start() + 0),
                crd.start() + 1,
                pf_sgate,
                nsid,
            )
        };

        // create child activity
        let tile = child.child_tile().tile_obj().clone();
        let mut act = ChildActivity::new_with(
            tile.clone(),
            ActivityArgs::new(child.name())
                .resmng(resmng_scap)
                .pager(
                    Pager::new(child_sess, pager_sgate, child_sgate)
                        .map_err(|e| rerror(e).context("creating child pager"))?,
                )
                .kmem(child.kmem()),
        )
        .map_err(|e| rerror(e).context("create activity"))?;

        // pass subsystem info to child, if it's a subsystem
        let id = child.id();
        if let Some(sub) = child.subsys() {
            sub.finalize_async(res, id, &mut act)?;
        }

        // mount file systems for childs
        for m in child.cfg().mounts() {
            let path = self.get_mount(m.fs())?;
            act.add_mount(m.path(), &path);
        }

        // if TileMux is running on that tile, we have control about the activity's virtual address
        // space and can thus load the program into the address space.
        let run = if tile.mux_type().unwrap() == MuxType::TileMux {
            // init address space (give it activity and mgate selector)
            let mut hdl = REQHDL.borrow_mut();
            let aspace = hdl.clients_mut().get_mut(child_sid).unwrap();
            aspace.do_init(Some(child.id()), Some(act.sel())).unwrap();

            // start activity
            let file = vfs::VFS::open(child.name(), vfs::OpenFlags::RX | vfs::OpenFlags::NEW_SESS)
                .map_err(|e| rerror(e).context(format!("open {}", child.name())))?;
            let mut mapper = mapper::ChildMapper::new(aspace, act.tile_desc().has_virtmem());

            // if we don't run the evidence service, nobody can ask for the hashes and therefore we
            // don't need to compute them.
            if EVREQHDL.is_some() {
                let mut rawfile = file.borrow();
                let size = rawfile
                    .stat()
                    .map_err(|e| rerror(e).context("stat for hash"))?
                    .size;
                // Acquire hash
                {
                    let rawfile_ref = rawfile.deref_mut();
                    let app_hash = get_hash(rawfile_ref, size)?;
                    log!(LogFlags::Debug, "hash of {}: {}", child.name(), app_hash);
                    child.set_hash(app_hash);
                }
            }

            act.exec_file(Some((&mut mapper, file.into_generic())), child.arguments())
                .map_err(|e| rerror(e).context(format!("execute {}", child.name())))?
        }
        else {
            act.exec_file(None, child.arguments())
                .map_err(|e| rerror(e).context("start Activity"))?
        };

        child.set_running(Box::new(run));

        Ok(())
    }

    fn configure_tile(
        &mut self,
        _res: &mut Resources,
        tile: &mut tiles::TileUsage,
        domain: &config::Domain,
    ) -> anyhow::Result<()> {
        assert!(!domain.tee(), "TEEs are currently unsupported by the pager");
        let fs_mod =
            MemCap::new_bind_bootmod("fs").map_err(|e| rerror(e).context("bind bootmod 'fs'"))?;
        let fs_mod_size = fs_mod.region().map_err(rerror)?.1 as usize;
        // don't overwrite PMP EPs here, but use the next free one. this is required in case we
        // share our tile with this child and therefore need to add a PMP EP for ourself. Since our
        // parent has already set PMP EPs, we don't want to overwrite them.
        tile.state_mut()
            .add_mem_region(fs_mod, fs_mod_size, true, false)
            .map_err(|e| e.context("add PMP EP for FS image"))
    }
}

#[allow(clippy::vec_box)]
struct WorkloopArgs<'t, 'c, 'd, 'r, 'q, 's, 'v> {
    starter: &'t mut PagedChildStarter,
    childmng: &'c mut ChildManager,
    childs: &'d mut Vec<Box<OwnChild>>,
    res: &'r mut Resources,
    reqs: &'q requests::Requests,
    serv: &'s mut Server,
    evserv: &'v mut Option<Server>,
}

fn workloop_async(args: &mut WorkloopArgs<'_, '_, '_, '_, '_, '_, '_>) {
    let WorkloopArgs {
        starter,
        childmng,
        childs,
        res,
        reqs,
        serv,
        evserv,
    } = args;

    reqs.run_loop_async(
        childmng,
        childs,
        res,
        |childmng, _res| {
            if evserv.is_some() {
                evserv
                    .as_mut()
                    .unwrap()
                    .fetch_and_handle(EVREQHDL.borrow_mut().deref_mut())
                    .ok();
                EVREQHDL
                    .borrow_mut()
                    .fetch_and_handle_msg_with(|_handler, opcode, sess, is| match opcode {
                        o if o == opcodes::Pager::Quote.into() => sess
                            .quote(is, childmng, ROT.borrow_mut().deref_mut())
                            .map_err(|e| e.downcast::<Error>().unwrap()),
                        _ => Err(Error::new(Code::InvArgs)),
                    });
            }

            serv.fetch_and_handle(REQHDL.borrow_mut().deref_mut()).ok();

            REQHDL.borrow_mut().fetch_and_handle_msg_with(
                |_handler, opcode, sess, is| match opcode {
                    o if o == opcodes::Pager::Pagefault.into() => sess.pagefault(childmng, is),
                    o if o == opcodes::Pager::MapAnon.into() => sess.map_anon(is),
                    o if o == opcodes::Pager::Unmap.into() => sess.unmap(is),
                    _ => Err(Error::new(Code::InvArgs)),
                },
            );
        },
        *starter,
    )
    .expect("Unable to run workloop");
}

#[no_mangle]
#[cfg_attr(dylint_lib = "m3_lints", allow(unexpected_async))]
pub fn main() -> anyhow::Result<()> {
    let (subsys, mut res) = subsys::Subsystem::new().expect("Unable to read subsystem info");

    let args = subsys.parse_args();
    for sem in &args.sems {
        res.semaphores_mut()
            .add_sem(sem.clone())
            .expect("Unable to add semaphore");
    }

    // mount root FS if we haven't done that yet
    let mut starter = PagedChildStarter::default();
    if vfs::VFS::stat("/").is_err() {
        vfs::VFS::mount("/", "m3fs", "m3fs").expect("Unable to mount root filesystem");
    }
    starter.mounts.push(("m3fs".to_string(), "/".to_string()));

    // create request handler and server
    let mut hdl = RequestHandler::new_with(args.max_clients, 128, 3)
        .expect("Unable to create request handler");
    let mut srv = Server::new_private("pager", &mut hdl).expect("Unable to create service");

    let mut evhdl: RequestHandler<EvidenceSession, opcodes::Pager> =
        RequestHandler::new_with(1, 128, 1).expect("couldn't create evidence req hdl");
    let mut evsrv = match Server::new("evidence", &mut evhdl) {
        Ok(result) => {
            EVREQHDL.set(evhdl);
            let rot = RoTSession::new("rot", &HashAlgorithm::SHA3_256)
                .expect("Couldn't open RoT session");
            ROT.set(rot);
            Some(result)
        },
        Err(_) => {
            log!(LogFlags::Debug, "Evidence service not found. Skipping...");
            drop(evhdl);
            None
        },
    };

    use opcodes::Pager;
    hdl.reg_cap_handler(Pager::Init, ExcType::Del(1), AddrSpace::init);
    hdl.reg_cap_handler(Pager::AddChild, ExcType::Obt(1), AddrSpace::add_child);
    hdl.reg_cap_handler(Pager::MapDS, ExcType::Del(1), AddrSpace::map_ds);
    hdl.reg_cap_handler(Pager::MapMem, ExcType::Del(1), AddrSpace::map_mem);
    REQHDL.set(hdl);

    let req_rgate = RecvGate::new(
        math::next_log2(256 * args.max_clients),
        math::next_log2(256),
    )
    .expect("Unable to create resmng RecvGate");
    let reqs = requests::Requests::new(req_rgate);

    let squeue_rgate = RecvGate::new(
        math::next_log2(sendqueue::RBUF_MSG_SIZE * args.max_clients),
        math::next_log2(sendqueue::RBUF_MSG_SIZE),
    )
    .expect("Unable to create sendqueue RecvGate");
    sendqueue::init(squeue_rgate);

    let mut childmng = childs::ChildManager::default();

    let mut childs = subsys
        .create_childs(&mut childmng, &mut res, &mut starter)
        .expect("Unable to start subsystem");

    let mut wargs = WorkloopArgs {
        starter: &mut starter,
        childmng: &mut childmng,
        childs: &mut childs,
        res: &mut res,
        reqs: &reqs,
        serv: &mut srv,
        evserv: &mut evsrv,
    };

    thread::init();
    for _ in 0..args.max_clients {
        #[cfg_attr(dylint_lib = "m3_lints", allow(async_alias))]
        thread::add_thread(
            VirtAddr::from(workloop_async as *const ()),
            &mut wargs as *mut _ as usize,
        );
    }

    wargs.childmng.start_waiting(1);

    workloop_async(&mut wargs);

    Ok(())
}

// Needed for Capability retention
struct EvidenceSession {
    _serv: ServerSession,
}

impl RequestSession for EvidenceSession {
    fn new(inserv: ServerSession, _arg: &str) -> Result<Self, Error> {
        let sess = Self { _serv: inserv };
        Ok(sess)
    }
}

type Signature = [u8; 64];

struct SigWrap(Signature);

impl fmt::LowerHex for SigWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl EvidenceSession {
    pub fn quote(
        &mut self,
        is: &mut GateIStream<'_>,
        childmgr: &ChildManager,
        rot: &RoTSession,
    ) -> anyhow::Result<()> {
        let nonce: usize = is.pop().map_err(rerror)?;
        let att_id: u32 = is.pop().map_err(rerror)?;

        let child = childmgr.child_by_attestation_id(att_id).ok_or_else(|| {
            rerrno(Code::NotFound).context(format!("child with attestation id {}", att_id))
        })?;

        let app_hash = child
            .hash()
            .ok_or_else(|| rerrno(Code::InvArgs).context("child hash"))?;
        let xml = child.cfg().to_string();
        let hash: String = format!("Hash:{}:{}:{}", nonce, app_hash, xml);
        log!(LogFlags::Debug, "Raw quote: {}", hash);
        let quote: [u8; 64] = rot
            .sign(hash.as_bytes())
            .map_err(|e| rerror(e).context("hash signature"))?;
        let quote_str = format!("{:x}", SigWrap(quote));

        reply_vmsg!(is, Code::Success, quote_str).map_err(|e| rerror(e).context("reply to quote"))
    }
}
