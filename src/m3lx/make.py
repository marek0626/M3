#!/usr/bin/env python3

import argparse
import os
import subprocess
import sys
from pathlib import Path
from multiprocessing import cpu_count
from typing import List, Optional, Dict

root = Path(__file__).resolve().parents[2]
lxbuild = root / "build/linux"
lxdeps = root / "src/m3lx"


def die(msg: str) -> None:
    print(msg, file=sys.stderr)
    exit(1)


def run(cmd: List[str], cwd: Optional[Path] = None, env: Optional[Dict[str, str]] = None) -> None:
    if os.getenv("M3_VERBOSE") == "1":
        print(">>>", " ".join(cmd), file=sys.stderr)
    subprocess.check_call(cmd, cwd=cwd, env=env)


def build_bbl(crossdir: Path, env: Dict[str, str], extra_args: List[str], jobs: int) -> None:
    bblbuild = Path("build/riscv-pk")
    bblbuild.mkdir(parents=True, exist_ok=True)

    run(
        [
            str(root / "src/m3lx/riscv-pk/configure"),
            "--host=riscv64-linux",
            f"--with-payload={root / 'build/linux' / 'vmlinux'}",
            "--with-mem-start=0x10004000",
        ],
        cwd=bblbuild,
        env={**env, "RISCV": str(crossdir)},
    )

    run(
        ["make", f"-j{jobs}"] + extra_args,
        cwd=bblbuild,
        env={**env, "CFLAGS": " -D__riscv_compressed=1"},
    )


def mklx(
    crossdir: Path,
    crossname: str,
    env: Dict[str, str],
    extra_args: List[str],
    jobs: int,
) -> None:
    makeargs = [f"O={lxbuild}", f"-j{jobs}"]
    lxbuild.mkdir(parents=True, exist_ok=True)

    env = {**env, "ARCH": "riscv", "CROSS_COMPILE": crossname}

    # generate config?
    if not (lxbuild / ".config").exists():
        run(
            ["make"] + makeargs + ["defconfig", "KBUILD_DEFCONFIG=sifive_defconfig"],
            cwd=lxdeps / "linux",
            env=env,
        )

    # build linux and bbl
    run(["make"] + makeargs + extra_args, cwd=lxdeps / "linux", env=env)
    build_bbl(crossdir, env, [], jobs)


def genlxcc(crossname: str, env: Dict[str, str], jobs: int) -> None:
    env = {**env, "ARCH": "riscv", "CROSS_COMPILE": crossname}
    out = root / "build/lxcc"
    linux_dir = lxdeps / "linux"

    run(["make", f"O={out}", "CC=clang", "defconfig"], cwd=linux_dir, env=env)
    run(["make", f"O={out}", "CC=clang", f"-j{jobs}"], cwd=linux_dir, env=env)

    run([str(linux_dir / "scripts/clang-tools/gen_compile_commands.py")], cwd=out, env=env)

    (out / "compile_commands.json").replace(linux_dir / "compile_commands.json")


parser = argparse.ArgumentParser(
    description="Wrapper for M³Linux build commands (mklx, genlxcc, mkbbl)."
)
parser.add_argument("crossname", help="Name of cross-compiler prefix")
parser.add_argument("crossdir", help="Path to cross compiler directory")
parser.add_argument("command", choices=["mklx", "genlxcc", "mkbbl"], help="Command to execute")
parser.add_argument('--jobs', '-j', help='Number of concurrent jobs (CPU count by default)')

args, rest = parser.parse_known_args()

if os.environ.get("M3_ISA") != "riscv64":
    die("Only supported on M3_ISA=riscv64.")

env = os.environ.copy()
env["PATH"] = str(root / args.crossdir / "bin") + ":" + env.get("PATH", "")

jobs = args.jobs or cpu_count() or 1

try:
    if args.command == "mklx":
        mklx(Path(args.crossdir), args.crossname, env, rest, jobs)
    elif args.command == "genlxcc":
        genlxcc(args.crossname, env, jobs)
    elif args.command == "mkbbl":
        build_bbl(Path(args.crossdir), env, rest, jobs)
    else:
        die(f"Unknown command: {args.command}")
except KeyboardInterrupt:
    pass
