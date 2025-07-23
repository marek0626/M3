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

#![no_std]

use m3core::boxed::Box;
use m3core::cap::Selector;
use m3core::client::ClientSession;
use m3core::com::{opcodes, MemCap};
use m3core::errors::Error;
use m3core::kif::{CapRngDesc, CapType};
use m3core::rc::Rc;
use m3core::tiles::Activity;
use m3core::vfs::{Fd, File, FileRef, OpenFlags};
use m3files::GenericFile;

/// Represents a session at the pipes server
///
/// The pipes server implements a uni-directional first-in-first-out communication channel with
/// multiple readers and writes and therefore provides the same semantics as anonymous pipes on
/// UNIX.
///
/// Note that [`IndirectPipe`](`crate::IndirectPipe`) provides a convenience layer on top of
/// this API.
pub struct Pipes {
    sess: ClientSession,
}

impl Pipes {
    /// Creates a new `Pipes` session at service with given name.
    pub fn new(name: &str) -> Result<Self, Error> {
        let sess = ClientSession::new(name)?;
        Ok(Pipes { sess })
    }

    /// Creates a new pipe using `mem` as shared memory for the data exchange.
    pub fn create_pipe(&self, mem: MemCap) -> Result<Pipe, Error> {
        let mem_size = mem.region()?.1;
        let crd = self.sess.obtain(
            1,
            |os| {
                os.push(opcodes::Pipe::OpenPipe);
                os.push(mem_size);
            },
            |_| Ok(()),
        )?;
        Pipe::new(mem, crd.start())
    }
}

/// Represents a pipe
///
/// A pipe allows to create *channels* that either write to the pipe or read from the pipe. To
/// exchange the data, the pipe requires memory, which is provided in form of a [`MemGate`].
pub struct Pipe {
    sess: ClientSession,
    mgate: MemCap,
}

impl Pipe {
    fn new(mem: MemCap, sel: Selector) -> Result<Self, Error> {
        let sess = ClientSession::new_owned_bind(sel);
        sess.delegate(
            CapRngDesc::new_single(CapType::Object, mem.sel()),
            |os| {
                os.push(opcodes::Pipe::SetMem);
            },
            |_| Ok(()),
        )?;
        Ok(Pipe { sess, mgate: mem })
    }

    /// Returns the session's capability selector.
    pub fn sel(&self) -> Selector {
        self.sess.sel()
    }

    /// Returns the [`MemCap`] used for the data exchange
    pub fn memory(&self) -> &MemCap {
        &self.mgate
    }

    /// Creates a new channel for this pipe. If `read` is true, it is a read-end, otherwise a
    /// write-end.
    pub fn create_chan(&self, read: bool) -> Result<Box<dyn File>, Error> {
        let crd = self.sess.obtain(
            2,
            |os| {
                os.push(opcodes::Pipe::OpenChan);
                os.push(read);
            },
            |_| Ok(()),
        )?;
        let flags = if read {
            OpenFlags::R | OpenFlags::NEW_SESS
        }
        else {
            OpenFlags::W | OpenFlags::NEW_SESS
        };
        Ok(Box::new(GenericFile::new(flags, crd.start(), None)))
    }
}

/// A uni-directional communication channel
///
/// The `IndirectPipe` provides a uni-directional first-in-first-out communication channel with
/// multiple readers and writes and therefore provides the same semantics as anonymous pipes on
/// UNIX. It is called indirect, because the communication between writer and reader happens
/// indirectly via the pipe server.
pub struct IndirectPipe {
    _pipe: Rc<Pipe>,
    rd_fd: Fd,
    wr_fd: Fd,
}

impl IndirectPipe {
    /// Creates a new pipe at the service with given name
    ///
    /// The argument `mem` specifies the memory region that should be used to exchange the data.
    /// Besides creating the pipe itself, two channels are created, one for reading and one for
    /// writing. The methods [`IndirectPipe::reader`] and [`IndirectPipe::writer`] provide access to
    /// these channels. In case one or both channels are delegated to another activity, the channel
    /// can be closed via [`IndirectPipe::close_reader`] or [`IndirectPipe::close_writer`].
    pub fn new(pipes: &Pipes, mem: MemCap) -> Result<Self, Error> {
        let pipe = Rc::new(pipes.create_pipe(mem)?);
        let mut files = Activity::own().files();
        let rd_fd = files.add(pipe.create_chan(true)?)?;
        let wr_fd = files.add(pipe.create_chan(false)?)?;
        Ok(IndirectPipe {
            rd_fd,
            wr_fd,
            _pipe: pipe,
        })
    }

    /// Returns the [`MemCap`] used for the data exchange
    pub fn memory(&self) -> &MemCap {
        self._pipe.memory()
    }

    /// Returns the file for the reading side
    pub fn reader(&self) -> Option<FileRef<dyn File>> {
        Activity::own().files().get_as(self.rd_fd)
    }

    /// Closes the reading side
    pub fn close_reader(&self) {
        Activity::own().files().remove(self.rd_fd);
    }

    /// Returns the file for the writing side
    pub fn writer(&self) -> Option<FileRef<dyn File>> {
        Activity::own().files().get_as(self.wr_fd)
    }

    /// Closes the writing side
    pub fn close_writer(&self) {
        Activity::own().files().remove(self.wr_fd);
    }
}

impl Drop for IndirectPipe {
    fn drop(&mut self) {
        self.close_reader();
        self.close_writer();
    }
}
