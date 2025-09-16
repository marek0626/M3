import argparse
import os
import sys

from pathlib import Path

from .utils import run, run_and_tee
from .context import Context
from . import command


@command(
    "run",
    [
        {"name": "cfg", "help": "the configuration to run (e.g., boot/hello.xml)"},
        {"name": "--gem5", "action": "store_true", "help": "force a run on gem5"},
    ]
)
def cmd_run(ctx: Context, args: argparse.Namespace) -> None:
    """
    Run the specified configuration on the current target platform.

    if `--gem5` is given, it will always be run on gem5, regardless of the current target.
    """
    env = os.environ.copy()
    if args.gem5:
        env["M3_RUN_GEM5"] = "1"
    if os.getenv("DBG_GEM5") == "1":
        run("python3", "-B", "./tools/execute/main.py", ctx.cross_prefix(), args.cfg, env=env)
    else:
        run_and_tee(
            "python3", "-B", "./tools/execute/main.py", ctx.cross_prefix(), args.cfg,
            env=env,
            log_file=ctx.out_dir / "log.txt",
        )


@command(
    "loadfpga",
    [
        {
            "name": "bitfile",
            "help": "FPGA bitfile to load (relative to platform/hw/fpga_tools/bitfiles/)",
        }
    ],
)
def cmd_loadfpga(ctx: Context, args: argparse.Namespace) -> None:
    """Load a bitfile onto the FPGA (only for hw targets)."""
    if ctx.target not in ("hw", "hw23"):
        sys.exit("loadfpga is only supported on hw targets.")

    for var in ("M3_HW_FPGA_HOST", "M3_HW_FPGA_DIR", "M3_HW_FPGA_NO"):
        if not os.getenv(var):
            sys.exit(f"environment variable {var} must be defined.")

    bitfile = Path("platform/hw/fpga_tools/bitfiles") / args.bitfile
    if not bitfile.is_file():
        sys.exit(f"bitfile {args.bitfile!r} does not exist.")

    # sync the bitfile and the programming script
    rsync_cmd = [
        "rsync",
        "-z",
        str(bitfile),
        "platform/hw/fpga_tools/scripts/program_fpga.tcl",
        f"{os.getenv('M3_HW_FPGA_HOST')}:{os.getenv('M3_HW_FPGA_DIR')}",
    ]
    run(*rsync_cmd)

    # invoke Vivado in batch mode
    jtag = os.getenv("M3_HW_FPGA_JTAG", "0")
    vivado = os.getenv("M3_HW_VIVADO")
    if not vivado:
        sys.exit("M3_HW_VIVADO must point to the Vivado installation.")
    remote_cmd = (
        f"{vivado} -mode batch -source {os.getenv('M3_HW_FPGA_DIR')}/program_fpga.tcl "
        f"-tclargs {os.getenv('M3_HW_FPGA_DIR')}/{args.bitfile} {jtag}"
    )
    run("ssh", str(os.getenv("M3_HW_FPGA_HOST")), remote_cmd)
