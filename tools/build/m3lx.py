import argparse

from .utils import run
from .context import Context
from . import command


@command("mklx")
def cmd_mklx(ctx: Context, args: argparse.Namespace) -> None:
    """
    (Re)build M³Linux (including bbl) via the buildroot script.

    Additional arguments are passed to `src/m3lx/make.py`. This allows to, for example, run
    `menuconfig` via `./b mklx menuconfig`.
    """
    run(
        "./src/m3lx/make.py",
        ctx.cross_name(),
        f"build/cross-{ctx.isa}/host",
        "mklx",
        *args.remainder,
    )


@command("mkbbl")
def cmd_mkbbl(ctx: Context, args: argparse.Namespace) -> None:
    """
    (Re)build the bbl bootloader.

    Additional arguments are passed to `src/m3lx/make.py.
    """
    run(
        "./src/m3lx/make.py",
        ctx.cross_name(),
        f"build/cross-{ctx.isa}/host",
        "mkbbl",
        *args.remainder,
    )


@command("genlxcc")
def cmd_genlxcc(ctx: Context, args: argparse.Namespace) -> None:
    """Generate ``compile_commands.json`` for M³Linux."""
    run(
        "./src/m3lx/make.py",
        ctx.cross_name(),
        f"build/cross-{ctx.isa}/host",
        "genlxcc",
    )
