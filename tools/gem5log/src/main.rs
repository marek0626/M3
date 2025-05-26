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

mod error;
mod flamegraph;
mod symbols;
mod trace;

use flamegraph::TileId;
use log::{Level, Log, Metadata, Record};
use std::env;
use std::io::Read;
use std::process::{exit, Command, Stdio};
use std::str::FromStr;

struct Logger {
    level: Level,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let level_string = record.level().to_string();
            let target = if !record.target().is_empty() {
                record.target()
            }
            else {
                record.module_path().unwrap_or_default()
            };

            eprintln!("{:<5} [{}] {}", level_string, target, record.args());
        }
    }

    fn flush(&self) {
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Mode {
    Trace,
    FlameGraph { start: u64, end: Option<u64> },
    FTrace { start: u64, end: Option<u64> },
    Snapshot { time: u64 },
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ISA {
    X86_64,
    RISCV32,
    RISCV64,
}

fn usage(prog: &str) -> ! {
    eprintln!(
        "Usage: {} (trace|flamegraph|snapshot <time>) [<binary>[+<offset>]...]",
        prog
    );
    exit(1)
}

fn determine_isa(file: &str) -> Result<ISA, error::Error> {
    let path = if file.contains("+0x") {
        let mut parts = file.split("+0x");
        parts.next().ok_or(error::Error::InvalPath)?
    }
    else {
        file
    };

    let mut cmd = Command::new("file")
        .arg("-b")
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = cmd.stdout.as_mut().unwrap();
    let mut res = String::new();
    stdout.read_to_string(&mut res)?;

    if res.contains("x86-64") {
        Ok(ISA::X86_64)
    }
    else if res.contains("32-bit") && res.contains("RISC-V") {
        Ok(ISA::RISCV32)
    }
    else if res.contains("64-bit") && res.contains("RISC-V") {
        Ok(ISA::RISCV64)
    }
    else {
        Err(error::Error::UnknownISA)
    }
}

fn main() -> Result<(), error::Error> {
    let level = Level::from_str(&env::var("RUST_LOG").unwrap_or_else(|_| "error".to_string()))?;
    log::set_boxed_logger(Box::new(Logger { level }))?;
    log::set_max_level(level.to_level_filter());

    let args: Vec<String> = env::args().collect();

    let (mode, bin_start) = match args.get(1) {
        Some(mode) if mode == "trace" => (Mode::Trace, 2),
        Some(mode) if mode == "flamegraph" || mode == "ftrace" => {
            if args.len() < 5 {
                usage(&args[0]);
            }
            let start = args.get(2).expect("Invalid arguments");
            let start = start.parse::<u64>().expect("Invalid start time");
            let end = args.get(3).expect("Invalid arguments");
            let end = end.parse::<u64>().expect("Invalid end time");
            let end = if end == 0 { None } else { Some(end) };
            if mode == "flamegraph" {
                (Mode::FlameGraph { start, end }, 4)
            }
            else {
                (Mode::FTrace { start, end }, 4)
            }
        },
        Some(mode) if mode == "snapshot" => {
            if args.len() < 4 {
                usage(&args[0]);
            }
            let time = args.get(2).expect("Invalid arguments");
            let time = time.parse::<u64>().expect("Invalid time");
            (Mode::Snapshot { time }, 3)
        },
        _ => usage(&args[0]),
    };

    let mut isa = None;
    let mut syms = symbols::Symbols::default();
    for f in &args[bin_start..] {
        let bin_isa = determine_isa(f)?;
        if let Some(isa) = isa {
            if !matches!(mode, Mode::Trace) && bin_isa != isa {
                panic!(
                    "Binaries with different ISAs are not supported for mode {:?}",
                    mode
                );
            }
        }
        isa = Some(bin_isa);

        let fsyms = symbols::parse_symbols(f)?;
        // TODO replace this simple heuristic with a more general and robust approach
        if f.ends_with("kernel") {
            syms.tiles.entry(TileId::new(0, 0)).or_default().push(fsyms);
        }
        else {
            syms.all.push(fsyms);
        }
    }

    match mode {
        Mode::Trace => trace::generate(&syms),
        Mode::FlameGraph { .. } | Mode::FTrace { .. } | Mode::Snapshot { .. } => {
            flamegraph::generate(mode, isa.unwrap(), &syms)
        },
    }
}
