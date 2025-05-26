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

use crate::client::ClientSession;
use crate::com::{opcodes, GateCap, MemCap, MemGate, RecvGate, SendGate, EP};
use crate::crypto::HashAlgorithm;
use crate::errors::{Code, Error};
use crate::mem::GlobOff;
use crate::serialize::bytes::{ByteBuf, Bytes};
use crate::vec::Vec;
use base::cfg;
use base::kif::{CapRngDesc, CapType, Perm};
use base::serialize::{Deserialize, Serialize};
use bitflags::bitflags;

pub struct RoTSession {
    sess: ClientSession,
    sgate: SendGate,
    secret_mem: Option<(MemGate, MemCap)>,
    ep: EP,
    algo: &'static HashAlgorithm,
}

// NMG Rustfmt misbehaves (???) and forces this formatting, so we have to skip
// it. Check again in the future if this is still true
#[rustfmt::skip]
bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Features : u8 {
        const RoT     = 0x1;
        const HashMux = 0x2;
    }
}

impl RoTSession {
    pub fn new(name: &str, algo: &'static HashAlgorithm) -> Result<Self, Error> {
        let sess = ClientSession::new(name)?;
        let sgate = sess.connect()?;
        let ep = sess.obtain(1, |is| is.push(opcodes::RoT::GetMem), |_| Ok(()))?;
        let mut rot_sess = RoTSession {
            sess,
            sgate,
            secret_mem: None,
            ep: EP::new_bind(0, ep.start()),
            algo,
        };

        let features = rot_sess.features();
        rot_sess.secret_mem = if features.unwrap().contains(Features::RoT) {
            // TODO for TEEs, this memory could be made exclusive
            let mem = MemCap::new(cfg::PAGE_SIZE as GlobOff, Perm::RW)?;
            let mem_derive = mem.derive(0, cfg::PAGE_SIZE as GlobOff, Perm::RW)?;
            let smem_opt = rot_sess
                .sess
                .delegate(
                    CapRngDesc::new(CapType::Object, mem_derive.sel(), 1).unwrap(),
                    |is| is.push(opcodes::RoT::SetSecretMem),
                    |_| Ok(()),
                )
                .ok();
            if smem_opt.is_some() {
                Some((mem.activate()?, mem_derive))
            }
            else {
                None
            }
        }
        else {
            None
        };

        rot_sess.reset(algo)?;
        Ok(rot_sess)
    }

    /// Returns the hash algorithm that is currently used for this hash session.
    pub fn algo(&self) -> &'static HashAlgorithm {
        self.algo
    }

    /// Returns the [`EP`] that should be configured with [`MemGate`](crate::com::MemGate)s for the
    /// input() and output() operation.
    pub fn ep(&self) -> &EP {
        &self.ep
    }

    pub fn features(&self) -> Result<Features, Error> {
        send_recv_res!(&self.sgate, RecvGate::def(), opcodes::RoT::Features)?.pop()
    }

    /// Reset the state of the hash session (discarding all previous input and output data) and
    /// change the [`HashAlgorithm`].
    pub fn reset(&mut self, algo: &'static HashAlgorithm) -> Result<(), Error> {
        send_recv_res!(&self.sgate, RecvGate::def(), opcodes::RoT::Reset, algo.ty).map(|_| ())?;
        self.algo = algo;
        Ok(())
    }

    /// Input new data into the state of the hash session.
    ///
    /// Before this is called, the [`ep`](RoTSession::ep) should be configured with a valid
    /// [`MemGate`](crate::com::MemGate) so that the hash multiplexer can successfully read `len`
    /// bytes with offset `off`.
    pub fn input(&self, off: usize, len: usize) -> Result<(), Error> {
        send_recv_res!(&self.sgate, RecvGate::def(), opcodes::RoT::Input, off, len).map(|_| ())
    }

    /// Output new data from the state of the hash session.
    ///
    /// Before this is called, the [`ep`](RoTSession::ep) should be configured with a valid
    /// [`MemGate`](crate::com::MemGate) so that the hash multiplexer can successfully write `len`
    /// bytes with offset `off`.
    ///
    /// Note that this operation does not allow output of more bytes than supported by the current
    /// hash algorithm. It is mainly intended for use with XOFs (extendable output functions) that
    /// allow arbitrarily large output, e.g. as pseudo-random number generator.
    pub fn output(&self, off: usize, len: usize) -> Result<(), Error> {
        if len > self.algo.output_bytes {
            return Err(Error::new(Code::InvArgs));
        }
        send_recv_res!(&self.sgate, RecvGate::def(), opcodes::RoT::Output, off, len).map(|_| ())
    }

    /// Finish the hash for previous [`input`](RoTSession::input) data. If successful, the hash is
    /// written to the `result` slice. Note that the ´result` slice must have exactly the size of
    /// `algo().output_bytes`, so this function cannot be used for XOFs (extendable output
    /// functions).
    pub fn finish(&self, result: &mut [u8]) -> Result<(), Error> {
        assert_eq!(result.len(), self.algo.output_bytes);
        send_recv!(self.sgate, RecvGate::def(), opcodes::RoT::Output).and_then(|mut reply| {
            // FIXME: Find a better way to copy out the slice?
            let msg = reply.msg();
            if msg.data.len() != self.algo.output_bytes {
                return Err(Error::new(reply.pop()?));
            }

            result.copy_from_slice(&msg.data);
            Ok(())
        })
    }

    pub fn read_rot_certificate(&self) -> Result<Vec<u8>, Error> {
        let mut off = 0;
        let mut size = 0;
        let mem = self.sess.obtain(
            1,
            |is| is.push(opcodes::RoT::GetRotCertificate),
            |os| {
                (off, size) = os.pop()?;
                Ok(())
            },
        )?;
        let mgate = MemGate::new_bind(mem.start())?;
        mgate.read_into_vec(size, off)
    }

    pub fn get_hash(&self) -> Result<Vec<u8>, Error> {
        Ok(
            send_recv_res!(self.sgate, RecvGate::def(), opcodes::RoT::GetHash)?
                .pop::<ByteBuf>()?
                .into_vec(),
        )
    }

    pub fn secret_mem(&self) -> Option<&MemGate> {
        self.secret_mem.as_ref().map(|(our, _rots)| our)
    }

    pub fn get_cdi(&self) -> Result<Vec<u8>, Error> {
        send_recv_res!(self.sgate, RecvGate::def(), opcodes::RoT::GetCdi)?.pop()
    }

    pub fn derive_secret(&self, custom: &str, size: usize) -> Result<(), Error> {
        send_recv_res!(
            self.sgate,
            RecvGate::def(),
            opcodes::RoT::DeriveSecret,
            custom,
            size
        )
        .map(|_| ())
    }

    pub fn read_derived_secret<const N: usize>(&self, custom: &str) -> Result<[u8; N], Error> {
        self.derive_secret(custom, N)?;
        if let Some(smem) = self.secret_mem() {
            smem.read_obj(0)
        }
        else {
            Err(Error::new(Code::NotSup))
        }
    }

    pub fn sign<const N: usize>(&self, bytes: &[u8]) -> Result<[u8; N], Error> {
        send_recv_res!(
            self.sgate,
            RecvGate::def(),
            opcodes::RoT::Certify,
            Bytes::new(bytes)
        )?
        .pop::<&[u8]>()?
        .try_into()
        .map_err(|_| Error::new(Code::InvArgs))
    }
}

/// A trait for objects that allow directly hashing the contents.
///
/// For example, this is implemented for files. The [`EP`] from the hash
/// multiplexer is delegated to M3FS and M3FS configures the [`EP`] accordingly
/// to let the hash multiplexer read the file contents directly.
pub trait HashInput {
    /// Input a single chunk of at most `len` bytes of this object into the [`RoTSession`].
    fn hash_input_chunk(&mut self, _sess: &RoTSession, _len: usize) -> Result<usize, Error> {
        Err(Error::new(Code::NotSup))
    }

    /// Input a maximum of `len` bytes of this object into the [`RoTSession`].
    fn hash_input(&mut self, sess: &RoTSession, len: usize) -> Result<usize, Error> {
        let mut remaining = len;
        while remaining > 0 {
            let amount = self.hash_input_chunk(sess, remaining)?;
            if amount == 0 {
                break;
            }
            remaining -= amount;
        }
        Ok(len - remaining)
    }
}

/// A trait for objects that allow directly writing hash output data.
///
/// For example, this is implemented for files. The [`EP`] from the hash
/// multiplexer is delegated to M3FS and M3FS configures the [`EP`] accordingly
/// to let the hash multiplexer write the file contents directly.
pub trait HashOutput {
    /// Output a single chunk of at most `len` bytes to this object from the [`RoTSession`].
    ///
    /// Note that this operation does not allow output of more bytes than
    /// supported by the current hash algorithm. It is mainly intended for
    /// use with XOFs (extendable output functions) that allow arbitrarily
    /// large output, e.g. as pseudo-random number generator.
    fn hash_output_chunk(&mut self, _sess: &RoTSession, _len: usize) -> Result<usize, Error> {
        Err(Error::new(Code::NotSup))
    }

    /// Output a maximum of `len` bytes to this object from the [`RoTSession`].
    ///
    /// Note that this operation does not allow output of more bytes than
    /// supported by the current hash algorithm. It is mainly intended for
    /// use with XOFs (extendable output functions) that allow arbitrarily
    /// large output, e.g. as pseudo-random number generator.
    fn hash_output(&mut self, sess: &RoTSession, len: usize) -> Result<usize, Error> {
        let mut remaining = len;
        while remaining > 0 {
            let amount = self.hash_output_chunk(sess, remaining)?;
            if amount == 0 {
                break;
            }
            remaining -= amount;
        }
        Ok(len - remaining)
    }
}
