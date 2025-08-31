import argparse

from .utils import run
from .context import Context
from . import command


@command("mkfs")
def cmd_mkfs(ctx: Context, args: argparse.Namespace) -> None:
    """Create an M³FS image.

    Additional arguments are passed to mkm3fs. For example, run it with `./b mkfs myimg.img
    src/fs/default 4096 1024 64`.
    """
    run(str(ctx.tool_dir / "mkm3fs"), *args.remainder)


@command("shfs")
def cmd_shfs(ctx: Context, args: argparse.Namespace) -> None:
    """
    Show the contents of an M³FS image.

    Additional arguments are passed to shm3fs. For example, run it with `./b shfs myimg.img sb`.
    """
    run(str(ctx.tool_dir / "shm3fs"), *args.remainder)


@command("fsck")
def cmd_fsck(ctx: Context, args: argparse.Namespace) -> None:
    """
    Perform a file system check with an M³FS image.

    Additional arguments are passed to m3fsck. For example, run it with `./b fsck myimg.img`.
    """
    run(str(ctx.tool_dir / "m3fsck"), *args.remainder)


@command("exfs")
def cmd_exfs(ctx: Context, args: argparse.Namespace) -> None:
    """
    Export the contents an M³FS image.

    Additional arguments are passed to exm3fs. For example, run it with `./b exfs myimg.img dest`.
    """
    run(str(ctx.tool_dir / "exm3fs"), *args.remainder)
