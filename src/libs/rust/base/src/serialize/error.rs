/*
 * Copyright (C) 2021 Mark Ueberall <mark.ueberall.1999@gmail.com>
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

use core::fmt::Display;

use crate::errors::{Code, Error};

#[derive(Debug, PartialEq)]
pub struct SerdeError;

impl From<SerdeError> for Error {
    fn from(_value: SerdeError) -> Self {
        Error::new(Code::DeserFailed)
    }
}

impl Display for SerdeError {
    fn fmt(&self, f: &mut _core::fmt::Formatter<'_>) -> _core::fmt::Result {
        write!(f, "(de)serialization failed")
    }
}

impl serde::ser::Error for SerdeError {
    fn custom<T: Display>(_msg: T) -> Self {
        // TODO use/pass-on the message somehow
        Self
    }
}

impl serde::de::Error for SerdeError {
    fn custom<T: Display>(_msg: T) -> Self {
        // TODO use/pass-on the message somehow
        Self
    }
}
