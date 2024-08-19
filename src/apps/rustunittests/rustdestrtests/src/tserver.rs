/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
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

use core::mem::size_of_val;

use m3::cap::{SelSpace, Selector};
use m3::cell::StaticCell;
use m3::client::ClientSession;
use m3::com::{
    recv_msg, GateIStream, RGateArgs, RecvGate, SGateArgs, Semaphore, SendCap, SendGate,
};
use m3::errors::{Code, Error};
use m3::kif::service::Request;
use m3::kif::{self, CapRngDesc, CapType};
use m3::mem::MsgBuf;
use m3::server::{
    server_loop, CapExchange, ClientManager, ExcType, RequestHandler, RequestSession, Server,
    ServerSession, SessId,
};
use m3::syscalls;
use m3::test::{DefaultWvTester, WvTester};
use m3::tiles::{Activity, ActivityArgs, ChildActivity, OwnActivity, RunningActivity, Tile};
use m3::{
    build_vmsg, send_vmsg, wv_assert, wv_assert_eq, wv_assert_err, wv_assert_ok, wv_require_ok,
    wv_run_test,
};

pub fn run(t: &mut dyn WvTester) {
    wv_run_test!(t, testnoresp);
    wv_run_test!(t, testcliexit);
    wv_run_test!(t, testcaps);
    wv_run_test!(t, testderive);
}

struct CrashSession {
    _serv: ServerSession,
}

impl RequestSession for CrashSession {
    fn new(_serv: ServerSession, _arg: &str) -> Result<Self, Error>
    where
        Self: Sized,
    {
        Ok(Self { _serv })
    }
}

impl CrashSession {
    fn dummy(
        _cli: &mut ClientManager<Self>,
        _crt: usize,
        _sid: SessId,
        _xchg: &mut CapExchange<'_>,
    ) -> Result<(), Error> {
        // don't respond, just exit
        OwnActivity::exit_with(Code::EndOfFile);
    }
}

fn server_crash_main() -> Result<(), Error> {
    let mut t = DefaultWvTester::default();

    let mut hdl = wv_require_ok!(RequestHandler::new());
    let mut srv = wv_require_ok!(Server::new("test", &mut hdl));

    hdl.reg_cap_handler(0usize, ExcType::Obt(1), CrashSession::dummy);

    wv_assert_ok!(t, hdl.run(&mut srv));

    Ok(())
}

pub fn open_sess(name: &str) -> ClientSession {
    // try to open a session until we succeed. this is required because we start the servers ourself
    // and don't know when they register their service.
    loop {
        let sess_res = ClientSession::new(name);
        if let Result::Ok(sess) = sess_res {
            break sess;
        }
    }
}

fn testnoresp(t: &mut dyn WvTester) {
    let client_tile = wv_require_ok!(Tile::get("compat|own"));
    let client = wv_require_ok!(ChildActivity::new_with(
        client_tile,
        ActivityArgs::new("client")
    ));

    let server_tile = wv_require_ok!(Tile::get("compat|own"));
    let cact = {
        let serv = wv_require_ok!(ChildActivity::new_with(
            server_tile,
            ActivityArgs::new("server")
        ));

        let sact = wv_require_ok!(serv.run(server_crash_main));

        let cact = wv_require_ok!(client.run(|| {
            let mut t = DefaultWvTester::default();
            let sess = open_sess("test");
            wv_assert_err!(
                t,
                sess.obtain(1, |is| is.push(0), |_| Ok(())),
                Code::ObjectGone
            );
            Ok(())
        }));

        wv_assert_eq!(t, sact.wait(), Ok(Code::EndOfFile));
        cact

        // destroy server activity to let the client request fail
    };

    // now wait for client
    wv_assert_eq!(t, cact.wait(), Ok(Code::Success));
}

fn testcliexit(t: &mut dyn WvTester) {
    let client_tile = wv_require_ok!(Tile::get("compat|own"));
    let mut client = wv_require_ok!(ChildActivity::new_with(
        client_tile,
        ActivityArgs::new("client")
    ));

    let server_tile = wv_require_ok!(Tile::get("compat|own"));
    let serv = wv_require_ok!(ChildActivity::new_with(
        server_tile,
        ActivityArgs::new("server")
    ));

    let sact = wv_require_ok!(serv.run(server_crash_main));

    let rg = wv_require_ok!(RecvGate::new_with(
        RGateArgs::default().order(7).msg_order(6)
    ));

    let sg = wv_require_ok!(SendCap::new_with(SGateArgs::new(&rg).credits(2)));
    wv_assert_ok!(t, client.delegate_obj(sg.sel()));

    let mut dst = client.data_sink();
    dst.push(sg.sel());

    let cact = wv_require_ok!(client.run(|| {
        let mut t = DefaultWvTester::default();

        let mut src = Activity::own().data_source();
        let sg_sel: Selector = src.pop().unwrap();

        let sess = loop {
            if let Ok(s) = ClientSession::new("test") {
                break s;
            }
        };

        // first send to activate the gate
        let sg = wv_require_ok!(SendGate::new_bind(sg_sel));
        wv_assert_ok!(t, send_vmsg!(&sg, RecvGate::def(), 1));

        // ensure that we drop MsgBuf before using send_vmsg below
        {
            // perform the obtain syscall
            let mut req_buf = MsgBuf::borrow_def();
            let mut args = kif::syscalls::ExchangeArgs::default();
            // insert opcode
            args.data[0] = 0;
            args.bytes = size_of_val(&args.data[0]);
            build_vmsg!(
                req_buf,
                kif::syscalls::Operation::ExchangeSess,
                kif::syscalls::ExchangeSess {
                    act: Activity::own().sel(),
                    sess: sess.sel(),
                    crd: kif::CapRngDesc::new_single(kif::CapType::Object, 0),
                    args,
                    obtain: true,
                }
            );
            wv_assert_ok!(t, syscalls::send_gate().send(&req_buf, RecvGate::syscall()));
        }

        // now we're ready to be killed
        wv_assert_ok!(t, send_vmsg!(&sg, RecvGate::def(), 1));

        // wait here; don't exit (we don't have credits anymore)
        loop {
            // note that we cannot have an empty loop here due to the bug in gem5's O3 model (with
            // x86 only?): jumps to the same instruction don't work, because the CPU executes the
            // next instruction anyway.
            OwnActivity::sleep().ok();
        }
    }));

    // wait until the child is ready to be killed
    wv_assert_ok!(t, recv_msg(&rg));
    wv_assert_ok!(t, recv_msg(&rg));

    wv_assert_eq!(t, sact.wait(), Ok(Code::EndOfFile));
    wv_assert_ok!(t, cact.stop());
}

static STOP: StaticCell<bool> = StaticCell::new(false);

struct NotSupSession {
    _serv: ServerSession,
    calls: u32,
}

impl RequestSession for NotSupSession {
    fn new(_serv: ServerSession, _arg: &str) -> Result<Self, Error>
    where
        Self: Sized,
    {
        Ok(Self { _serv, calls: 0 })
    }
}

impl NotSupSession {
    fn fivetimes(
        cli: &mut ClientManager<Self>,
        _crt: usize,
        sid: SessId,
        _xchg: &mut CapExchange<'_>,
    ) -> Result<(), Error> {
        let sess = cli.get_mut(sid).unwrap();
        sess.calls += 1;
        // stop the service after 5 calls
        if sess.calls == 5 {
            STOP.set(true);
        }
        Err(Error::new(Code::NotSup))
    }
}

fn server_notsup_main() -> Result<(), Error> {
    for _ in 0..5 {
        STOP.set(false);

        let mut hdl = wv_require_ok!(RequestHandler::new());
        let srv = wv_require_ok!(Server::new("test", &mut hdl));

        hdl.reg_cap_handler(0usize, ExcType::Obt(1), NotSupSession::fivetimes);
        hdl.reg_cap_handler(1usize, ExcType::Del(1), NotSupSession::fivetimes);

        let res = server_loop(|| {
            if STOP.get() {
                return Err(Error::new(Code::ObjectGone));
            }

            srv.fetch_and_handle(&mut hdl)?;
            hdl.fetch_and_handle_msg();

            Ok(())
        });
        match res {
            // if there is any other error than our own stop signal, break
            Err(e) if e.code() != Code::ObjectGone => break,
            _ => {},
        }
    }

    Ok(())
}

fn testcaps(t: &mut dyn WvTester) {
    let server_tile = wv_require_ok!(Tile::get("compat|own"));
    let serv = wv_require_ok!(ChildActivity::new_with(
        server_tile,
        ActivityArgs::new("server")
    ));
    let sact = wv_require_ok!(serv.run(server_notsup_main));

    for i in 0..5 {
        let sess = open_sess("test");

        // test both obtain and delegate
        if i % 2 == 0 {
            for _ in 0..5 {
                wv_assert_err!(t, sess.obtain(1, |is| is.push(0), |_| Ok(())), Code::NotSup);
            }
            wv_assert_err!(
                t,
                sess.obtain(1, |is| is.push(0), |_| Ok(())),
                Code::InvArgs,
                Code::RecvGone
            );
        }
        else {
            let crd = CapRngDesc::new_single(CapType::Object, sess.sel());
            for _ in 0..5 {
                wv_assert_err!(
                    t,
                    sess.delegate(crd, |is| is.push(1), |_| Ok(())),
                    Code::NotSup
                );
            }
            wv_assert_err!(
                t,
                sess.delegate(crd, |is| is.push(1), |_| Ok(())),
                Code::InvArgs,
                Code::RecvGone
            );
        }
    }

    wv_assert_err!(t, ClientSession::new("test"), Code::InvArgs);
    wv_assert_eq!(t, sact.wait(), Ok(Code::Success));
}

fn server_derive_main() -> Result<(), Error> {
    let mut src = Activity::own().data_source();
    let sig_sem = Semaphore::bind(src.pop().unwrap());
    let srv_sel: Selector = src.pop().unwrap();

    let mut t = DefaultWvTester::default();

    let rgate = wv_require_ok!(RecvGate::new(7, 7));
    wv_assert_ok!(t, syscalls::create_srv(srv_sel, rgate.sel(), "dummy", 0));

    // notify our parent that the service is available
    wv_assert_ok!(t, sig_sem.up());

    // wait until we receive a request from the kernel
    let msg = wv_require_ok!(rgate.receive(None));
    let mut is = GateIStream::new(msg, &rgate);
    // this should always be the DeriveCrt request
    let req: Request<'_> = wv_require_ok!(is.pop());
    wv_assert!(t, matches!(req, Request::DeriveCrt { sessions: _ }));

    // now first revoke the derived service caps
    wv_assert_ok!(
        t,
        syscalls::revoke(
            Activity::own().sel(),
            CapRngDesc::new_single(CapType::Object, srv_sel),
            false,
        )
    );

    // finish the derive_srv call (which fails now as we revoked the service)
    let scap = wv_require_ok!(SendCap::new(&rgate));
    wv_assert_err!(
        t,
        syscalls::derive_srv_fin(srv_sel, Code::Success, scap.sel(), 0),
        Code::InvCap
    );

    // and then reply
    wv_assert_ok!(t, is.reply_error(Code::Success));

    Ok(())
}

fn testderive(t: &mut dyn WvTester) {
    let sig_sem = wv_require_ok!(Semaphore::create(0));
    let srv_sel = SelSpace::get().alloc_sel();

    let server_tile = wv_require_ok!(Tile::get("compat|own"));
    let mut serv = wv_require_ok!(ChildActivity::new_with(
        server_tile,
        // ensure that srv_sel is not reused by the child
        ActivityArgs::new("server").first_sel(srv_sel + 1)
    ));

    wv_assert_ok!(t, serv.delegate_obj(sig_sem.sel()));

    let mut dst = serv.data_sink();
    dst.push(sig_sem.sel());
    dst.push(srv_sel);

    let sact = wv_require_ok!(serv.run(server_derive_main));

    // wait until service is ready
    wv_assert_ok!(t, sig_sem.down());

    // obtain service cap from child
    let our_sel = wv_require_ok!(sact.activity().obtain_obj(srv_sel));

    // perform derive_srv call on this service cap
    let sels = SelSpace::get().alloc_sels(2);
    wv_assert_ok!(
        t,
        syscalls::derive_srv_req(our_sel, sels.start() + 0, sels.start() + 1, 1, 0xDEAD_BEEF)
    );

    // wait for upcall
    let msg = wv_require_ok!(RecvGate::upcall().receive(None));
    let mut is = GateIStream::new(msg, RecvGate::upcall());
    // this should be a derive service upcall
    let _opcode: kif::upcalls::Operation = wv_require_ok!(is.pop());
    let resp: kif::upcalls::DeriveSrv = wv_require_ok!(is.pop());
    wv_assert_eq!(t, resp.error, Code::InvCap);

    wv_assert_eq!(t, sact.wait(), Ok(Code::Success));
}
