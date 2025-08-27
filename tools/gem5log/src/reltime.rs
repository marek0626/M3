/*
 * Copyright (C) 2025 Nils Asmussen, Barkhausen Institut
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

use std::io::Write;
use std::io::{self, BufRead};

use crate::error::Error;

fn repl_line(mut writer: impl Write, line: &str, last: Option<u64>) -> Option<u64> {
    let mut parts = line.trim_start().splitn(2, ':');
    let time = parts.next()?.parse::<u64>().ok()?;
    let rest = parts.next()?;

    let last = last.unwrap_or(0);
    let diff = time - last;
    write!(writer, "{:+16}: {}", diff, rest).ok()?;

    Some(time)
}

pub fn generate() -> Result<(), Error> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());

    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());

    let mut last = None;
    let mut line = String::new();
    while reader.read_line(&mut line)? != 0 {
        // try to replace the address with the binary and symbol
        match repl_line(&mut writer, &line, last) {
            Some(time) => {
                last = Some(time);
            },
            None => {
                // if that failed, just write out the line
                writer.write_all(line.as_bytes())?;
            },
        }
        line.clear();
    }
    Ok(())
}
