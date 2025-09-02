#!/usr/bin/env python3

import os
import subprocess
from pathlib import Path

PROGS = (
    "basename cat cp cut date dd dirname du find head ln ls "
    "mkdir mktemp mv printenv printf pwd rm rmdir sleep stat sync "
    "tail tee test tr uniq wc"
).split()
SUFFIXES = ("1", "6", "8")

SRC = Path("src/apps/bsdutils/src")
DEST = Path("src/fs/default/man")
DEST.mkdir(parents=True, exist_ok=True)

env = os.environ.copy()
env["MANWIDTH"] = "100"

for prog in PROGS:
    for sfx in SUFFIXES:
        src = SRC / prog / f"{prog}.{sfx}"
        if src.is_file():
            out = subprocess.run(
                ["man", "--ascii", "-E", "ascii", str(src)],
                env=env,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            (DEST / f"{prog}.1").write_text(out, encoding="utf-8")
