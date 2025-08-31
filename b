#!/usr/bin/env python3

import argparse
import sys

from pathlib import Path
from textwrap import fill, dedent

# add plugin path
PLUGIN_DIR = Path(__file__).with_name("tools") / "build"
sys.path.insert(0, str(PLUGIN_DIR.parent))

from build import Context, load_commands  # noqa: E402

# load all commands
commands = load_commands()

# create context with environment variables etc.
ctx = Context()

# build argument parser
parser = argparse.ArgumentParser(prog="./b")
parser.add_argument("-n", "--no-build", action="store_true",
                    help="skip the build step and execute the command directly")
subparsers = parser.add_subparsers(dest="command", metavar="<command>")

cmd_groups = {}

# add subparsers for plugins
for name, cmd in commands.items():
    help_msg = (cmd.func.__doc__.strip() or "").splitlines()[0] if cmd.func.__doc__ else ""
    sp = subparsers.add_parser(name, help=help_msg, description=cmd.func.__doc__)
    if cmd.args:
        for arg in cmd.args:
            name = arg.pop("name")
            sp.add_argument(name, **arg)
    if cmd.group not in cmd_groups:
        cmd_groups[cmd.group] = []
    cmd_groups[cmd.group] += [(cmd.name, help_msg)]
    sp.set_defaults(_func=cmd.func)

# add options as shown by argparse
cmd_groups["Options"] = [
    ("-h, --help", "show this help message and exit"),
    ("-n, --no-build", "skip the build step"),
]


# custom help text to show commands in groups and list environment variables
def help_text():
    print("usage: ./b [-h] [-n] <command> ...")
    print(dedent(
        """
        This is a convenience script that is responsible for building everything and running
        the specified command afterwards. The most important environment variables that
        influence its behaviour are M3_TARGET=(gem5|hw|hw22|hw23), M3_ISA=(x86_64|riscv32|riscv64)
        [on gem5 only], and M3_BUILD=(debug|release|bench).
        """
    ), end="")

    # commands
    for group in cmd_groups:
        print(f"\n{group}:")
        for cmd, doc in cmd_groups[group]:
            print(f"  {cmd:18}{doc}")

    # general variables
    general = {
        "M3_TARGET":   "the target platform: 'gem5', 'hw', 'hw22', or 'hw23', default is 'gem5'.",
        "M3_ISA":      "the ISA to use. On gem5, 'riscv32', 'riscv64', and 'x86_64' are supported. "
                       "On other targets it is ignored.",
        "M3_BUILD":    "the build type is 'debug', 'release', or 'bench'. "
                       "debug: optimizations disabled, debug info & assertions active. "
                       "release: everything disabled. bench: logging forced to Info,Error. "
                       "default is release.",
        "M3_VERBOSE":  "print executed commands in detail during build.",
        "M3_MOD_PATH": "the path for boot modules (build directory by default).",
        "M3_OUT":      "the output directory ('run' by default).",
        "M3_LOG":      "comma-separated log flags for M³ (default: 'Info,Error').",
    }

    # gem5‑specific variables
    gem5 = {
        "M3_GEM5_CORES":      "number of cores to simulate.",
        "M3_GEM5_HDD":        "hard‑drive image to use (filename only).",
        "M3_GEM5_LOG":        "log flags for gem5 (--debug-flags).",
        "M3_GEM5_LOGSTART":   "when to start logging for gem5 (--debug-start).",
        "M3_GEM5_CFG":        "gem5 configuration (platform/gem5/configs/m3/default.py "
                              "by default).",
        "M3_GEM5_CPU":        "CPU model (DerivO3CPU by default).",
        "M3_GEM5_CPUFREQ":    "CPU frequency (1GHz by default).",
        "M3_GEM5_MEMFREQ":    "memory frequency (333MHz by default).",
        "M3_GEM5_PAUSE":      "pause the tile with given id until GDB connects (only with dbg). "
                              "Numbers become C0T<number>, or use 'C<chip>T<tile>'.",
    }

    # hw / hw22 / hw23‑specific variables
    hw = {
        "M3_HW_FPGA_HOST":    "SSH alias for the FPGA PC.",
        "M3_HW_FPGA_DIR":     "temporary directory on the FPGA PC (created automatically).",
        "M3_HW_FPGA_NO":      "FPGA number; IP = 192.168.42.240 + $M3_HW_FPGA_NO.",
        "M3_HW_FPGA_JTAG":    "FPGA JTAG cable number (default = 0).",
        "M3_HW_VIVADO":       "absolute path on FPGA PC to Vivado/Vivado Lab.",
        "M3_HW_TTY":          "TTY device for the serial console (for M³Lx).",
        "M3_HW_RESET":        "reset the FPGA before starting.",
        "M3_HW_VM":           "use virtual memory (default = 1).",
        "M3_HW_TIMEOUT":      "stop execution after given number of seconds.",
        "M3_HW_PAUSE":        "pause the tile with given number at startup (only with dbg).",
    }

    def fmt_desc(name: str, desc: str, width: int) -> str:
        return fill(
            desc,
            width=width,
            initial_indent="  " + f"{name:18}",
            subsequent_indent=" " * 20,
            break_long_words=False,
            replace_whitespace=True,
        )

    def print_vars(title: str, data: dict) -> None:
        print()
        print(title)
        for name, desc in data.items():
            print(fmt_desc(name, desc, 80))

    print_vars("General environment variables:", general)
    print_vars("Environment variables for target gem5:", gem5)
    print_vars("Environment variables for target hw/hw22/hw23:", hw)


parser.print_help = help_text

args, remainder = parser.parse_known_args()
args.remainder = remainder

try:
    # Build step (unless disabled)
    if not args.no_build:
        from build.build import ensure_built
        ensure_built(ctx)

    # run command
    func = getattr(args, "_func", None)
    if func is not None:
        func(ctx, args)
except KeyboardInterrupt:
    print("\nGot ^C. Stopping here")
    sys.exit(1)
