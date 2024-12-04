/*
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
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

use m3::col::{String, Vec};
use m3::com::Semaphore;
use m3::errors::Code;
use m3::io::LogFlags;
use m3::{format, log};

use crate::{rerrno, rerror};

#[derive(Default)]
pub struct SemManager {
    sems: Vec<(String, Semaphore)>,
}

impl SemManager {
    pub const fn new() -> Self {
        SemManager { sems: Vec::new() }
    }

    pub fn add_sem(&mut self, name: String) -> anyhow::Result<()> {
        if self.get(&name).is_some() {
            return Err(
                rerrno(Code::Exists).context(format!("semaphore with name {} exists", name))
            );
        }

        let sem = Semaphore::create(0).map_err(|e| rerror(e).context("semaphore create"))?;
        log!(
            LogFlags::ResMngSem,
            "Created semaphore {} @ {}",
            name,
            sem.sel()
        );
        self.sems.push((name, sem));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Semaphore> {
        for (sname, sem) in &self.sems {
            if sname == name {
                return Some(sem);
            }
        }
        None
    }
}
