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

use log::{debug, error, trace, warn};
use once_cell::sync::Lazy;
use std::collections::{btree_map, BTreeMap, HashMap};
use std::fmt::{self, Display};
use std::io::{self, BufRead, StdoutLock, Write};
use std::sync::Mutex;

use crate::error::Error;
use crate::symbols;

const STACK_SIZE: u64 = 0x20000;
const UNKNOWN_STACK: StackId = 0;
const DEF_ACT_ID: u16 = 0xFFFF;
const IDLE_ACT_ID: u16 = 0xFFFE;

static NEXT_TID: Lazy<Mutex<u8>> = Lazy::new(|| Mutex::new(0));

fn next_tid() -> u8 {
    let mut next_tid = NEXT_TID.lock().unwrap();
    *next_tid += 1;
    *next_tid - 1
}

#[derive(Copy, Clone, Default, Debug, Hash, PartialEq, Eq)]
pub struct TileId {
    id: u16,
}

impl TileId {
    pub const fn new(chip: u8, tile: u8) -> Self {
        Self {
            id: (chip as u16) << 8 | tile as u16,
        }
    }

    pub const fn chip(&self) -> u8 {
        (self.id >> 8) as u8
    }

    pub const fn tile(&self) -> u8 {
        (self.id & 0xFF) as u8
    }
}

impl fmt::Display for TileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "C{}T{:02}", self.chip(), self.tile())
    }
}

struct Tile<'n> {
    id: TileId,
    bins: BTreeMap<&'n str, Binary<'n>>,
    last_bin: &'n str,
    last_isr_exit: bool,
    susp_start: u64,
    old_act: Option<u16>,
    new_act: Option<u16>,
}

type StackId = u64;

#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct ThreadId<'n> {
    bin: &'n str,
    tid: u8,
    // the first thread of every binary should have the tile id as pid, which we remember here
    pid: Option<u32>,
}

struct Binary<'n> {
    name: &'n str,
    tids: BTreeMap<StackId, ThreadId<'n>>,
    stacks: BTreeMap<ThreadId<'n>, Thread<'n>>,
    cur_stack: StackId,
}

#[derive(Default)]
struct Thread<'n> {
    stack: Vec<Call<'n>>,
    switched: u64,
    last_func: usize,
    last_addr: usize,
}

#[derive(Debug)]
struct Call<'n> {
    func: &'n str,
    addr: usize,
    org_time: u64,
    time: u64,
    /// Time spent in the called subroutines
    child_duration: u64,
}

impl fmt::Display for ThreadId<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(fmt, "{} [tid={}]", self.bin, self.tid)
    }
}

fn get_func_addr(line: &str) -> Option<(u64, TileId, Option<usize>)> {
    // get the first parts:
    // 7802000: C0T00.cpu: T0 : 0x226f3a @ heap_init+26    : mov rcx, DS:[rip + 0x295a7]
    // ^------^ ^--------^ ^^ ^ ^------^ ^---------------------------------------------^
    let mut parts = line.trim_start().splitn(6, ' ');
    let time = parts.next()?;
    let cpu = parts.next()?;
    if !cpu.starts_with('C') {
        return None;
    }

    let time_int = time[..time.len() - 1].parse::<u64>().ok()?;
    let chip_int = cpu[1..2].parse::<u8>().ok()?;
    let tile_int = cpu[3..5].parse::<u8>().ok()?;
    let tile_int = TileId::new(chip_int, tile_int);
    let addr_int = if cpu.ends_with(".cpu:") {
        let addr = parts.nth(2)?;
        let mut addr_parts = addr.splitn(2, '.');
        let addr = addr_parts.next()?;
        if let Some(addr) = addr.strip_prefix("0x") {
            usize::from_str_radix(addr, 16).ok()
        }
        else {
            usize::from_str_radix(addr, 16).ok()
        }
    }
    else {
        None
    };

    Some((time_int, tile_int, addr_int))
}

impl<'n> Tile<'n> {
    fn new(mode: crate::Mode, time: u64, id: TileId, bin: Binary<'n>) -> Self {
        let name = bin.name;

        if matches!(mode, crate::Mode::FTrace { .. }) {
            let tid = bin.cur_tid();
            ftrace::print(
                time,
                id,
                Some(DEF_ACT_ID),
                tid,
                name,
                ftrace::tswitch(id, (DEF_ACT_ID, tid), (DEF_ACT_ID, tid)),
            );
        }

        let mut bins = BTreeMap::new();
        bins.insert(bin.name, bin);
        Tile {
            id,
            bins,
            last_bin: name,
            last_isr_exit: false,
            susp_start: 0,
            old_act: None,
            new_act: None,
        }
    }

    fn finish(&mut self, mode: crate::Mode, time: u64) {
        if matches!(mode, crate::Mode::FTrace { .. }) {
            let act = self.new_act.unwrap_or(DEF_ACT_ID);
            let tid = self.cur_bin().unwrap().cur_tid();
            ftrace::print(
                time,
                self.id,
                self.new_act,
                tid,
                self.last_bin,
                ftrace::tswitch(self.id, (act, tid), (act, tid)),
            );
        }
    }

    fn cur_bin(&mut self) -> Option<&mut Binary<'n>> {
        self.bins.get_mut(self.last_bin)
    }

    fn binary_switch(&mut self, sym: &'n symbols::Symbol, time: u64) {
        if let Some(prev) = self.cur_bin() {
            prev.cur_thread().suspend(time);
        }

        match self.bins.entry(&*sym.bin) {
            btree_map::Entry::Vacant(entry) => {
                debug!("{}: new binary {}", time, sym.bin);
                entry.insert(Binary::new(&sym.bin, None));
            },
            btree_map::Entry::Occupied(_) => {
                debug!("{}: switched to {}", time, sym.bin);
            },
        }

        self.last_bin = &sym.bin;
        if let Some(next) = self.cur_bin() {
            next.cur_thread().resume(time);
        }
    }

    fn suspend(&mut self, now: u64) {
        self.susp_start = now;
        debug!("{}: {}: sleep begin", now, self.id);
    }

    fn resume(&mut self, now: u64) {
        let duration = now - self.susp_start;
        debug!("{}: {}: sleep end ({})", now, self.id, duration);

        if self.susp_start > 0 {
            for bin in self.bins.values_mut() {
                for thread in bin.stacks.values_mut() {
                    if thread.switched != 0 {
                        thread.switched += duration;
                    }
                    for f in &mut thread.stack {
                        f.time += duration;
                    }
                }
            }
            self.susp_start = 0;
        }
    }

    fn snapshot(&self) {
        println!("{}:", self.id);
        for bin in self.bins.values() {
            let cur_tid = bin.cur_tid();
            for (tid, thread) in &bin.stacks {
                // ignore empty threads
                if thread.stack.is_empty() {
                    continue;
                }

                if self.last_bin == bin.name && *tid == cur_tid {
                    println!("  \x1B[1mThread {}:\x1B[0m", tid);
                }
                else {
                    println!("  Thread {}:", tid);
                }

                for frame in &thread.stack {
                    println!(
                        "    {:#x} {} (called at {})",
                        frame.addr, frame.func, frame.org_time
                    );
                }
                println!();
            }
        }
        println!();
    }
}

impl<'n> Binary<'n> {
    fn new(name: &'n str, pid: Option<u32>) -> Self {
        let cur_tid = ThreadId {
            bin: name,
            tid: next_tid(),
            pid,
        };
        let cur_stack = UNKNOWN_STACK;
        let mut stacks = BTreeMap::new();
        stacks.insert(cur_tid, Thread::default());
        let mut tids = BTreeMap::new();
        tids.insert(cur_stack, cur_tid);
        Binary {
            name,
            stacks,
            cur_stack,
            tids,
        }
    }

    fn cur_tid(&self) -> ThreadId<'n> {
        *self.tids.get(&self.cur_stack).unwrap()
    }

    fn cur_thread(&mut self) -> &mut Thread<'n> {
        self.stacks.get_mut(&self.cur_tid()).unwrap()
    }

    fn found_stack(&mut self, stack: u64, time: u64) {
        assert_eq!(self.cur_stack, UNKNOWN_STACK);
        let nid = stack - STACK_SIZE;
        let tid = self.tids.remove(&self.cur_stack).unwrap();
        self.tids.insert(nid, tid);
        self.cur_stack = nid;
        debug!("{}: found stack of {} -> {}", time, self.cur_stack, tid);
    }

    fn thread_switch(
        &mut self,
        mode: crate::Mode,
        tile: TileId,
        act: Option<u16>,
        mut stack: u64,
        time: u64,
    ) {
        let old_tid = self.cur_tid();
        self.cur_thread().suspend(time);

        // try to find the thread with new stack
        match self.tids.range(..=&stack).nth_back(0) {
            Some((sid, tid)) if stack >= *sid && stack < *sid + STACK_SIZE => {
                // we know the stack, switch to it
                self.cur_stack = *sid;
                debug!("{}: switched back to {}", time, tid);
            },
            _ => {
                // create new stack
                stack -= STACK_SIZE;
                self.cur_stack = stack;
                let tid = ThreadId {
                    bin: self.name,
                    tid: next_tid(),
                    pid: None,
                };
                self.tids.insert(self.cur_stack, tid);
                self.stacks.insert(tid, Thread::default());
                debug!("{}: new thread {}", time, tid);
            },
        }

        if matches!(mode, crate::Mode::FTrace { .. }) {
            let new_tid = self.cur_tid();
            let sw_act = act.unwrap_or(DEF_ACT_ID);
            ftrace::print(
                time,
                tile,
                act,
                old_tid,
                self.name,
                ftrace::tswitch(tile, (sw_act, old_tid), (sw_act, new_tid)),
            );
        }

        self.cur_thread().resume(time);
    }
}

impl<'n> Thread<'n> {
    fn depth(&self) -> usize {
        self.stack.len() * 2
    }

    fn suspend(&mut self, time: u64) {
        self.switched = time;
    }

    fn resume(&mut self, time: u64) {
        // shift the start time of all calls by the time other threads ran
        let duration = time - self.switched;
        for f in &mut self.stack {
            f.time += duration;
        }
        self.switched = 0;
    }

    fn call(
        &mut self,
        mode: crate::Mode,
        tile: TileId,
        act: Option<u16>,
        sym: &'n symbols::Symbol,
        time: u64,
        tid: ThreadId<'_>,
    ) {
        let w = self.depth();
        trace!("{}: {} {:w$} CALL -> {}", time, tid, "", sym.name, w = w);
        if matches!(mode, crate::Mode::FTrace { .. }) {
            ftrace::mark(tile, act, time, tid, &sym.bin, &sym.name, true);
        }
        self.stack.push(Call {
            func: &sym.name,
            addr: sym.addr,
            org_time: time,
            time,
            child_duration: 0,
        });
    }

    fn ret(
        &mut self,
        mode: crate::Mode,
        tile: TileId,
        act: Option<u16>,
        sym: &symbols::Symbol,
        time: u64,
        tid: ThreadId<'_>,
    ) -> Option<Call<'n>> {
        if !self.stack.iter().any(|s| s.func == sym.name) {
            error!(
                "{}: {} return to {} w/o preceeding call",
                time, tid, sym.name
            );
            return None;
        }

        // unwind the stack until we find the function on the stack that matches the current symbol
        let mut last = self.stack_pop(time).unwrap();
        loop {
            if matches!(mode, crate::Mode::FTrace { .. }) {
                ftrace::mark(tile, act, time, tid, &sym.bin, last.func, false);
            }
            match self.stack.last() {
                Some(f) if f.func == sym.name => {
                    let w = self.depth();
                    trace!("{}: {} {:w$} RET  -> {}", time, tid, "", sym.name, w = w);
                    return Some(last);
                },
                _ => last = self.stack_pop(time).unwrap(),
            }
        }
    }

    /// Remove last call from stack and calculate [`Call::child_duration`] for parent
    fn stack_pop(&mut self, time: u64) -> Option<Call<'n>> {
        let last = self.stack.pop();
        if let Some((last, parent)) = last.as_ref().zip(self.stack.last_mut()) {
            let duration = time - last.time;
            parent.child_duration += duration;
        }
        last
    }
}

fn instr_is_sp_assign(isa: crate::ISA, line: &str) -> bool {
    // find the "first" instruction that tells us the stack pointer
    match isa {
        crate::ISA::X86_64 => line.contains("subi   rsp, rsp, 0x8"),
        crate::ISA::RISCV32 | crate::ISA::RISCV64 => {
            line.contains("c_addi sp, -") || line.contains("c_addi16sp sp, -")
        },
    }
}

fn instr_is_sp_init(isa: crate::ISA, line: &str) -> bool {
    // find the specific line in thread_resume that inits the stack pointer
    match isa {
        crate::ISA::X86_64 => line.contains("ld   rsp, DS:[rdi + 0x8]"),
        crate::ISA::RISCV32 => line.contains("lw sp, 8(a1)"),
        crate::ISA::RISCV64 => line.contains("ld sp, 16(a1)"),
    }
}

fn is_isr_exit(isa: crate::ISA, line: &str) -> bool {
    match isa {
        crate::ISA::X86_64 => line.contains("IRET_PROT : wrip   , t0, t1"),
        crate::ISA::RISCV32 | crate::ISA::RISCV64 => line.contains("sret"),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_return<'t, 'i: 't>(
    mode: crate::Mode,
    wr: &mut StdoutLock<'_>,
    time: u64,
    tile: TileId,
    act: Option<u16>,
    sym: &symbols::Symbol,
    thread: &mut Thread<'t>,
    tid: ThreadId<'i>,
    unwind: bool,
) -> Result<(), Error> {
    if !thread.stack.is_empty() {
        // generate stack
        let stack = match mode {
            crate::Mode::FlameGraph { start, .. } if time >= start => {
                use std::fmt::Write;
                let mut stack: String = format!("{}", tile);
                stack.push(';');
                write!(stack, "{}", tid).unwrap();
                for f in thread.stack.iter() {
                    stack.push(';');
                    stack.push_str(f.func);
                }
                Some(stack)
            },
            _ => None,
        };

        let last = if unwind {
            thread.ret(mode, tile, act, sym, time, tid)
        }
        else {
            let call = thread.stack_pop(time);
            if let Some(ref call) = call {
                if matches!(mode, crate::Mode::FTrace { .. }) {
                    ftrace::mark(tile, act, time, tid, &sym.bin, call.func, false);
                }
            }
            call
        };

        if let Some(stack) = stack {
            // print flamegraph line
            if let Some(l) = last {
                let duration = time - l.time;
                writeln!(wr, "{} {}", stack, (duration - l.child_duration) / 1000)?;
            }
        }
    }
    Ok(())
}

pub fn generate(mode: crate::Mode, isa: crate::ISA, syms: &symbols::Symbols) -> Result<(), Error> {
    let mut last_time = 0;
    let mut tiles: HashMap<TileId, Tile<'_>> = HashMap::new();

    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());

    let stdout = io::stdout();
    let mut wr = stdout.lock();

    let mut line = String::new();
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {},
            Err(_) => continue,
        }

        if let Some((time, tile, maybe_addr)) = get_func_addr(&line) {
            match mode {
                crate::Mode::FlameGraph { end: Some(end), .. } if (time >= end) => {
                    break;
                },
                crate::Mode::FTrace { end: Some(end), .. } if (time >= end) => {
                    break;
                },
                crate::Mode::Snapshot {
                    time: snapshot_time,
                } => {
                    if time >= snapshot_time {
                        println!("Snapshot at timestamp {}:", time);
                        for t in tiles.keys() {
                            if let Some(tile) = tiles.get(t) {
                                tile.snapshot();
                            }
                        }
                        break;
                    }
                },
                _ => {},
            }

            let time = if time >= last_time { time } else { last_time };
            last_time = time;

            if maybe_addr.is_none() {
                if let Some(cur_tile) = tiles.get_mut(&tile) {
                    ftrace::parse_misc_line(mode, cur_tile, &line, time);
                }

                line.clear();
                continue;
            }

            let addr = maybe_addr.unwrap();
            if let Some(sym) = symbols::resolve(tile, syms, addr) {
                // detect tiles
                tiles.entry(tile).or_insert_with(|| {
                    Tile::new(
                        mode,
                        time,
                        tile,
                        Binary::new(&sym.bin, Some(ftrace::group(tile))),
                    )
                });
                let cur_tile = tiles.get_mut(&tile).unwrap();

                // detect ISR exits
                if cur_tile.last_isr_exit {
                    let obin = cur_tile.bins.get_mut::<str>(cur_tile.last_bin).unwrap();
                    let tid = obin.cur_tid();
                    let othread = obin.stacks.get_mut(&tid).unwrap();
                    handle_return(
                        mode,
                        &mut wr,
                        time,
                        tile,
                        cur_tile.new_act,
                        sym,
                        othread,
                        tid,
                        false,
                    )?;
                }
                // detect binary changes (e.g., tilemux to app)
                let bin_switch = sym.bin != cur_tile.last_bin;
                if bin_switch {
                    cur_tile.binary_switch(sym, time);
                }

                let cur_bin = cur_tile.bins.get_mut::<str>(&sym.bin).unwrap();

                // detect the stack pointer
                if cur_bin.cur_stack == UNKNOWN_STACK && instr_is_sp_assign(isa, &line) {
                    if let Some(pos) = line.find("D=") {
                        let tid = u64::from_str_radix(&line[(pos + 4)..(pos + 20)], 16)?;
                        cur_bin.found_stack(tid, time);
                    }
                }

                // detect thread switches
                if sym.name == "thread_switch_async" && instr_is_sp_init(isa, &line) {
                    if let Some(pos) = line.find("D=") {
                        let tid = u64::from_str_radix(&line[(pos + 4)..(pos + 20)], 16)?;
                        cur_bin.thread_switch(mode, cur_tile.id, cur_tile.new_act, tid, time);
                    }
                }

                let cur_tid = cur_bin.cur_tid();
                let cur_thread = cur_bin.cur_thread();

                // function changed?
                if (bin_switch || !cur_tile.last_isr_exit)
                    && sym.addr != cur_thread.last_func
                        // we also want to handle cases like ISRs where the last instruction of the
                        // handler leads to a binary switch and we enter the handler from the top
                        // again next time.
                        || (addr == sym.addr && addr != cur_thread.last_addr)
                {
                    // it's a call when we jumped to the beginning of a function
                    if addr == sym.addr {
                        cur_thread.call(mode, cur_tile.id, cur_tile.new_act, sym, time, cur_tid);
                    }
                    // otherwise it's a return
                    else if sym.name != "thread_switch_async" && cur_thread.stack.is_empty() {
                        error!("{}: return with empty stack", time);
                    }
                    else {
                        handle_return(
                            mode,
                            &mut wr,
                            time,
                            tile,
                            cur_tile.new_act,
                            sym,
                            cur_thread,
                            cur_tid,
                            true,
                        )?;
                    }
                }

                cur_tile.last_isr_exit = is_isr_exit(isa, &line);
                cur_thread.last_func = sym.addr;
                cur_thread.last_addr = addr;
            }
            else {
                warn!("{}: No symbol for address {:#x}", time, addr);
            }
        }

        line.clear();
    }

    for tile in tiles.values_mut() {
        tile.finish(mode, last_time);
    }

    Ok(())
}

mod ftrace {
    use super::*;

    pub fn print<S: Display>(
        time: u64,
        tile_id: TileId,
        act: Option<u16>,
        thread: ThreadId,
        binary: &str,
        payload: S,
    ) {
        let nsecs = time / 1000;
        let tile = tile_id.tile(); // TODO support chip id
        let pid = pid(tile_id, act.unwrap_or(DEF_ACT_ID), thread);
        println!(
            " {0:>30}  ({1:>5}) [{2:03}] .... {3}.{4:09}: {5}",
            format!("{}-{}", binary, pid),
            ftrace::group(tile_id),
            tile,
            nsecs / 1_000_000_000,
            nsecs % 1_000_000_000,
            payload,
        );
    }

    pub fn mark(
        tile: TileId,
        act: Option<u16>,
        time: u64,
        tid: ThreadId<'_>,
        bin: &str,
        func: &str,
        enter: bool,
    ) {
        print(
            time,
            tile,
            act,
            tid,
            bin,
            format!(
                "tracing_mark_write:{}|{}|{}",
                if enter { 'B' } else { 'E' },
                ftrace::group(tile),
                clean_name(func)
            ),
        );
    }

    pub fn tswitch(tile: TileId, old: (u16, ThreadId<'_>), new: (u16, ThreadId<'_>)) -> String {
        format!(
            "sched_switch: prev_comm={} prev_pid={} prev_prio=0 prev_state=S ==> next_comm={} next_pid={} next_prio=0",
            name(old.0, tile),
            pid(tile, old.0, old.1),
            name(new.0, tile),
            pid(tile, new.0, new.1),
        )
    }

    pub fn group(tile: TileId) -> u32 {
        // pid 0 and 1 are special, so start with 1000
        1000 + tile.tile() as u32
    }

    pub fn parse_misc_line(mode: crate::Mode, cur_tile: &mut Tile<'_>, line: &str, time: u64) {
        let mut seen_susres = 0;
        if line.contains("tcu.connector: Suspending core") {
            cur_tile.suspend(time);
            seen_susres = 1;
        }
        else if line.contains("tcu.connector: Waking up core") {
            cur_tile.resume(time);
            seen_susres = 2;
        }
        else if line.contains("tcu.regFile: TCU-> PRI[CUR_ACT") {
            let value = line.split("0x").nth(1).unwrap();
            // extract the last 16 bit from the value
            cur_tile.new_act = Some(u16::from_str_radix(&value[12..16], 16).unwrap());
        }
        else if line.contains("tcu.regFile: TCU-> PRI[PRIV_CMD_ARG") {
            let value = line.split("0x").nth(1).unwrap();
            cur_tile.old_act = Some(u16::from_str_radix(&value[12..16], 16).unwrap());
        }
        else if line.contains("Finished privileged command XCHG_ACT") {
            if let (Some(old), Some(new)) = (cur_tile.old_act, cur_tile.new_act) {
                let tid = cur_tile.cur_bin().unwrap().cur_tid();
                let switch_tid = ThreadId {
                    bin: tid.bin,
                    tid: tid.tid,
                    pid: None,
                };
                if matches!(mode, crate::Mode::FTrace { .. }) {
                    print(
                        time,
                        cur_tile.id,
                        cur_tile.new_act,
                        tid,
                        cur_tile.last_bin,
                        tswitch(cur_tile.id, (old, switch_tid), (new, switch_tid)),
                    );
                }
            }
        }

        if seen_susres > 0 && matches!(mode, crate::Mode::FTrace { .. }) {
            // we use the frequency only to distinguish between 'suspended' and 'active'
            let state = if seen_susres == 1 { 0 } else { 1000000 };
            print(
                time,
                cur_tile.id,
                cur_tile.new_act,
                cur_tile.cur_bin().unwrap().cur_tid(),
                cur_tile.last_bin,
                format!(
                    "cpu_frequency: state={} cpu_id={}",
                    state,
                    cur_tile.id.tile()
                ),
            );
        }
    }

    fn clean_name(name: &str) -> String {
        name.replace(
            |c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_'),
            "_",
        )
    }

    fn name(act_id: u16, tile: TileId) -> String {
        match (tile.tile(), act_id) {
            (0, _) => String::from("kernel"),
            (_, DEF_ACT_ID) => String::from("tilemux"),
            (_, IDLE_ACT_ID) => String::from("idle"),
            (_, id) => format!("activity-{}", id),
        }
    }

    fn pid(tile: TileId, act_id: u16, tid: ThreadId<'_>) -> u32 {
        if let Some(pid) = tid.pid {
            return pid;
        }

        ((tile.tile() as u32) << 24) + ((act_id as u32) << 8) + (tid.tid as u32)
    }
}
