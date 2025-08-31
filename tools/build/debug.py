import argparse
import os
import select
import subprocess
import sys
import time

from pathlib import Path

from .utils import run, popen, paginate
from .context import Context
from . import command


@command(
    "dbg",
    [
        {"name": "prog", "help": "program binary to debug"},
        {"name": "cfg", "help": "boot configuration to run"},
    ],
)
def cmd_dbg(ctx: Context, args: argparse.Namespace) -> None:
    """Debug a program (on gem5 or FPGA).

    On gem5, M3_GEM5_PAUSE needs to be set to the tile to debug (e.g., C0T20 or just 20). For the
    FPGA, M3_HW_PAUSE needs to be set to the tile number (e.g., 4).
    """
    prog = ctx.bin_dir / args.prog
    cfg = Path(args.cfg)
    if not prog.is_file():
        sys.exit(f"program {args.prog!r} not found in {ctx.bin_dir}")
    if not cfg.is_file():
        sys.exit(f"script {args.cfg!r} not found under boot/")

    if ctx.target == "gem5" or os.getenv("M3_RUN_GEM5") == "1":
        _dbg_gem5(ctx, prog, cfg)
    else:
        _dbg_hw(ctx, prog, cfg)


def _dbg_gem5(ctx: Context, prog: Path, cfg: Path) -> None:
    # TODO make that a CLI argument
    if not os.getenv("M3_GEM5_PAUSE"):
        sys.exit("M3_GEM5_PAUSE must be set to the tile to debug.")

    # start M³ on gem5 in the background
    log_path = ctx.out_dir / "log.txt"
    log = log_path.open("w")
    proc = popen(
        "python3", "-B", "./tools/execute/main.py", ctx.cross_prefix(), str(cfg), "--debug",
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    log.close()

    try:
        # wait for the port to appear in the log
        port = _find_gdb_port(log_path, 10)
        if not port:
            return

        # build a temporary GDB command file
        gdb_cmd = ctx.out_dir / "gdbcmd.tmp"
        gdb_cmd.write_text(
            f"target remote localhost:{port}\n"
            "display/i $pc\n"
            "b main\n"
        )

        # start GDB
        env = os.environ.copy()
        env["RUST_GDB"] = ctx.cross_prefix(prog) + "gdb"
        run(
            "rust-gdb", "--tui", str(prog), f"--command={gdb_cmd}",
            env=env,
        )
    finally:
        proc.terminate()
        if gdb_cmd:
            gdb_cmd.unlink()


def _find_gdb_port(log_path: Path, timeout: int) -> int | None:
    port = ""
    tile = (
        os.getenv("M3_GEM5_PAUSE")
        if "C" in os.getenv("M3_GEM5_PAUSE")
        else f"C0T{int(os.getenv('M3_GEM5_PAUSE')):02d}"
    )

    last_activity = time.time()
    # wait until we know the port it's listening on for GDB
    with log_path.open("r", encoding="utf-8", buffering=1) as f:
        fd = f.fileno()
        while not port:
            line = f.readline()
            if line:
                last_activity = time.time()
                line = line.rstrip("\n")
                if not port and f"{tile}.remote_gdb" in line:
                    return line.split()[-1]
            else:
                # No data, block until new data or timeout
                now = time.time()
                remaining = timeout - (now - last_activity)
                if remaining <= 0:
                    print("Timeout reached, exiting.")
                    return None
                rlist, _, _ = select.select([fd], [], [], remaining)
                if not rlist:
                    # Timeout expired with no data
                    print("Timeout reached, exiting.")
                    return None


def _dbg_hw(ctx: Context, prog: Path, cfg: Path):
    if not os.getenv("M3_HW_PAUSE"):
        sys.exit("M3_HW_PAUSE must be set for hardware debugging.")

    # start M³ on the FPGA in the background
    proc = popen(
        "python3", "-B", "./tools/execute/main.py", str(ctx.cross_prefix), str(cfg), "--debug",
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    # forward GDB port via SSH
    port = 3340 + int(os.getenv("M3_HW_PAUSE"))
    host = os.getenv("M3_HW_FPGA_HOST")
    ssh_cmd = ["ssh", "-N", "-L", f"30000:localhost:{port}", host]
    ssh_proc = popen(*ssh_cmd, stderr=subprocess.DEVNULL)
    try:
        # wait until the remote side is listening
        print(f"Connecting to {host}:{port} ...", end="", flush=True)
        for _ in range(6):
            try:
                telnet = run(
                    "telnet", "localhost", "30000",
                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
                )
                if b"+" in telnet.stdout:
                    break
            except subprocess.CalledProcessError:
                pass
            time.sleep(1)
            print(".", end="", flush=True)
        else:
            sys.exit("\nUnable to connect to remote GDB port.")

        # build gdb command file
        gdb_cmd = ctx.out_dir / "gdbcmd.tmp"
        gdb_cmd.write_text(
            "target remote localhost:30000\n"
            "set $t0 = 0\n"
            "set $pc = 0x10004000\n"
        )

        try:
            # decide whether we are debugging a bare‑metal binary or an user application
            rdelf = ctx.cross_prefix(prog) + "readelf"
            entry = (
                run(str(rdelf), "-h", str(prog), text=True, capture=True).stdout
                .split("Entry point address:")[1]
                .split()[0]
            )
            if entry == "0x10004000":
                gdb_cmd.write_text(gdb_cmd.read_text() + "b env_run\n")
                symbols = prog
            else:
                gdb_cmd.write_text(
                    gdb_cmd.read_text()
                    + "tb __app_start\nc\n"
                    f"symbol-file {prog}\n"
                    "b main\n"
                )
                symbols = ctx.bin_dir / "tilemux"
            gdb_cmd.write_text(gdb_cmd.read_text() + "display/i $pc\n")

            # start GDB
            env = os.environ.copy()
            env["RUST_GDB"] = ctx.cross_prefix(symbols) + "gdb"
            run(
                "rust-gdb", "--tui", str(symbols), f"--command={gdb_cmd}",
                env=env
            )
        finally:
            gdb_cmd.unlink()
    finally:
        proc.terminate()
        ssh_proc.terminate()


@command(
    "bt",
    [{"name": "prog", "help": "binary for which a backtrace should be printed"}],
)
def cmd_bt(ctx: Context, args: argparse.Namespace) -> None:
    """
    Enriches a backtrace with symbols from `prog`.
    Expects the backtrace in stdin.
    """
    binary = ctx.bin_dir / args.prog
    run("./tools/backtrace.py", ctx.cross_prefix(binary), str(binary))


@command(
    "hwitrace",
    [{"name": "progs", "help": "comma‑separated list of program names"}],
)
def cmd_hwitrace(ctx: Context, args: argparse.Namespace) -> None:
    """
    Enriches a hardware instruction trace with symbols from given programs.

    Expects the trace in stdin (e.g., run/pm0-instr.log).
    """
    paths = [str(ctx.bin_dir / p) for p in args.progs.split(",")]
    cross = ctx.cross_prefix(paths[0])
    paginate(str(ctx.tool_dir / "hwitrace"), cross, *paths)


@command(
    "trace",
    [
        {"name": "progs", "help": "comma‑separated list of program names"},
        {"name": "--m3lx", "action": "store_true", "help": "trace M³Linux binaries"},
    ],
)
def cmd_trace(ctx: Context, args: argparse.Namespace) -> None:
    """
    Enriches a gem5 instruction trace with symbols from given programs.

    Optionally, each binary can end with '+<offset>', which will be added as a base address to all
    symbols. Expects the trace in stdin obtained with Exec in M3_GEM5_LOG. If `--m3lx` is given,
    symbols for Linux and bbl are added automatically and programs are expected in the lxbin
    directory.
    """
    if args.m3lx:
        paths = [
            str(ctx.root / "build/linux/vmlinux"),
            str(ctx.root / "build/riscv-pk/bbl"),
        ]
        for prog in args.progs.split(","):
            paths += [f"{ctx.build_dir / "lxbin" / prog}+0x2AAAAAA000"]
    else:
        paths = [str(ctx.bin_dir / p) for p in args.progs.split(",")]
    paginate(str(ctx.tool_dir / "gem5log"), "trace", *paths)


@command(
    "flamegraph",
    [
        {"name": "progs", "help": "comma‑separated list of program names"},
        {"name": "--start", "default": "0", "help": "start timestamp (default 0)"},
        {"name": "--end", "default": "0", "help": "end timestamp (default 0)"},
    ],
)
def cmd_flamegraph(ctx: Context, args: argparse.Namespace) -> None:
    """
    Generate a flamegraph from a gem5 log.

    Expects the gem5.log in stdin with at least M3_GEM5_LOG=Exec,TcuConnector.
    """
    paths = [str(ctx.bin_dir / p) for p in args.progs.split(",")]
    proc = popen(
        str(ctx.tool_dir / "gem5log"), "flamegraph", args.start, args.end, *paths,
        stdout=subprocess.PIPE,
    )
    run("inferno-flamegraph", "--countname", "ns", stdin=proc.stdout)


@command(
    "ftrace",
    [
        {"name": "progs", "help": "comma‑separated list of program names"},
        {"name": "--start", "default": "0", "help": "start timestamp (default 0)"},
        {"name": "--end", "default": "0", "help": "end timestamp (default 0)"},
    ],
)
def cmd_ftrace(ctx: Context, args: argparse.Namespace) -> None:
    """
    Generate an ftrace from a gem5 log.

    The trace can be fed into tools like Perfetto or Trace Compass. Expects the gem5.log in stdin
    with at least M3_GEM5_LOG=Exec,TcuConnector.
    """
    paths = [str(ctx.bin_dir / p) for p in args.progs.split(",")]
    run(str(ctx.tool_dir / "gem5log"), "ftrace", args.start, args.end, *paths)


@command(
    "snapshot",
    [
        {"name": "progs", "help": "comma‑separated list of program names"},
        {"name": "time", "help": "timestamp at which to take the snapshot"},
    ],
)
def cmd_snapshot(ctx: Context, args: argparse.Namespace) -> None:
    """
    Print a stack‑trace snapshot for `progs` at `time`.

    Expects the gem5.log in stdin with at least M3_GEM5_LOG=Exec.
    """
    paths = [str(ctx.bin_dir / p) for p in args.progs.split(",")]
    run(str(ctx.tool_dir / "gem5log"), "snapshot", args.time, *paths)
