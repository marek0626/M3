import argparse
import os

from .utils import run
from .context import Context
from . import command


def ensure_built(ctx: Context) -> None:
    """Run the default ninjapie build."""
    print(f"Building for {ctx.target}-{ctx.isa}-{ctx.build}...", flush=True)
    env = os.environ.copy()
    env["NPBUILD"] = str(ctx.build_dir)

    # collect ninja and ninjapie arguments
    ninja_args = []
    ninjapie_args = []
    verbose = os.getenv("M3_VERBOSE", "0")
    if verbose == "1":
        ninja_args += ["-v"]

    # force regeneration of the ninja build file if the verbosity level changed since last run
    vfile = ctx.build_dir / ".verbose"
    if not vfile.exists() or vfile.read_text() != f"M3_VERBOSE={verbose}":
        ninjapie_args += ["build", "-f"]
    vfile.write_text(f"M3_VERBOSE={verbose}")

    run("python3", "-B", str(ctx.ninjapie), *ninjapie_args, "--", *ninja_args, env=env)


@command("clean")
def cmd_clean(ctx: Context, _: argparse.Namespace) -> None:
    """remove the current build directory for the active target/ISA/build."""
    run("rm", "-rf", str(ctx.build_dir))
    # also clean the rust debug/release sub‑dirs
    run("rm", "-rf", str(ctx.rust_build / "debug"), str(ctx.rust_build / "release"))


@command("distclean")
def cmd_distclean(_: Context, __: argparse.Namespace) -> None:
    """remove the whole build tree (including the cross‑compiler)."""
    run("rm", "-rf", "build")


@command("ninja")
def cmd_ninja(ctx: Context, args: argparse.Namespace) -> None:
    """run ninja with the given arguments."""
    run("python3", "-B", str(ctx.ninjapie), "--", *args.remainder, check=False)


@command("mkgem5", [
    {"name": "isas", "nargs": "?", "default": "X86,RISCV", "help": "comma‑separated ISA list"},
    {"name": "--debug", "action": "store_true", "help": "build the debug version of gem5"},
])
def cmd_mkgem5(ctx: Context, args: argparse.Namespace) -> None:
    """(re)build the gem5 simulator for the given ISA list."""
    suffix = "debug" if args.debug else "opt"
    isas = [x.strip() for x in args.isas.split(",")]
    isas = [f"build/{isa}/gem5.{suffix}" for isa in isas]

    gem5_dir = ctx.root / "build/gem5"
    gem5_dir.mkdir(parents=True, exist_ok=True)
    run(
        "scons", f"-j{os.cpu_count()}", "-C", str(ctx.root / "platform/gem5"), *isas,
        cwd=gem5_dir
    )
