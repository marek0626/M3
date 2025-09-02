#!/usr/bin/env python3

import os
import re
import subprocess
import sys

from collections import OrderedDict
from shlex import quote
from typing import Optional

if len(sys.argv) != 3:
    sys.exit("Usage: {} <crossprefix> <binary>".format(sys.argv[0]))

crossprefix = sys.argv[1]
binary = sys.argv[2]

regex_symbol = re.compile(r'^([0-9a-fA-F]*)\s+([BdDdTtVvWwuU])\s+(.*)$')
regex_btline = re.compile(r'^(?:.*?\[[^\]]+\])?\s*(?:0x)?([0-9a-f]+)\s*$')
regex_sanbtline = re.compile(r'^\s*#\d+\s+0x([0-9a-f]+).*')


class Symbol:
    def __init__(self, addr: int, section: str, name: str) -> None:
        self.addr = addr
        self.section = section
        self.name = name


def get_location(addr: int) -> str:
    cmd = ["addr2line", "-e", binary, "{:#x}".format(addr)]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    assert proc.stdout, "Pipe creation failed"
    line_bytes = proc.stdout.readline()
    line = line_bytes.decode(errors='ignore')
    pwd = os.environ.get('PWD')
    if pwd:
        return line.replace(pwd, '.')
    return line


def find_sym(addr: int) -> Optional[Symbol]:
    last_addr = 0
    for s in syms:
        if s >= addr:
            return syms[last_addr] if last_addr != 0 else None
        last_addr = s
    return None


def print_func(addr: int) -> None:
    # hack for Linux: currently, we generate PIE binaries and thus, Linux puts code and data at
    # weird addresses. with setarch -R, Linux uses the fixed offset 0x555555554000.
    if "/host-" in binary:
        addr -= 0x555555554000

    sym = find_sym(addr)
    if not sym:
        return

    loc = get_location(addr)
    print(" {:#x} {}({}) + {:#x} = {:#x} in {}"
          .format(addr, sym.name, sym.section, addr - sym.addr, sym.addr, loc))


# scan binary
syms = {}
cmd = "{}nm {} | c++filt".format(quote(crossprefix), quote(binary))
proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, shell=True)
assert proc.stdout, "Pipe creation failed"
for line_bytes in proc.stdout.readlines():
    line = line_bytes.strip().decode(errors='ignore')
    match = regex_symbol.match(line)
    if match:
        addr = int(match.group(1), 16)
        sec = match.group(2)
        sym = match.group(3)
        syms[addr] = Symbol(addr, sec, sym)

# sort symbols by address
syms = OrderedDict(sorted(syms.items(), key=lambda t: t[0]))

print("Scanning binary done, reading backtrace from stdin...")

# decode backtrace
for line in sys.stdin:
    match = regex_btline.match(line.strip())
    if not match:
        match = regex_sanbtline.match(line.strip())
    if match:
        print_func(int(match.group(1), 16))
