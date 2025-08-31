import argparse
import re
import subprocess
import sys

from pathlib import Path

from .utils import paginate, run
from .context import Context
from . import command


@command(
    "dis",
    [{"name": "prog", "help": "binary to disassemble"}],
)
def cmd_dis(ctx: Context, args: argparse.Namespace) -> None:
    """Disassemble `prog` with objdump."""
    binary = ctx.bin_dir / args.prog
    paginate(ctx.cross_prefix(binary) + "objdump", "-dC", str(binary))


@command(
    "elf",
    [{"name": "prog", "help": "binary to inspect with readelf"}],
)
def cmd_elf(ctx: Context, args: argparse.Namespace) -> None:
    """Run `readelf -aW` on `prog` and pipe through `c++filt`."""
    binary = ctx.bin_dir / args.prog
    cmd = [ctx.cross_prefix(binary) + "readelf", "-aW", str(binary)]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    assert proc.stdout
    paginate("c++filt", stdin=proc.stdout.fileno())


@command(
    "nm",
    [
        {"name": "prog", "help": "binary to run `nm -SCn` on"},
        {"name": "--size", "action": "store_true", "help": "show symbols sorted by size"},
    ],
)
def cmd_nm(ctx: Context, args: argparse.Namespace) -> None:
    """Run `nm -SCn` on `prog`."""
    binary = ctx.bin_dir / args.prog
    if args.size:
        options = ["-SC", "--size-sort"]
    else:
        options = ["-SCn"]
    paginate(str(ctx.cross_prefix(binary) + "nm"), *options, str(binary))


@command(
    "ctors",
    [{"name": "prog", "help": "binary for which constructors are shown"}],
)
def cmd_ctors(ctx: Context, args: argparse.Namespace) -> None:
    """Show the constructors (.ctors/.init_array) of `prog`."""
    binary = ctx.bin_dir / args.prog
    if not binary.is_file():
        sys.exit(f"{args.prog!r} not found.")

    isa = ctx.isa_for_binary(binary)
    cross = ctx.cross_prefix(binary)

    # find the .ctors/.init_array section
    section = (
        run(str(cross) + "readelf", "-SW", str(binary), capture=True)
        .stdout
        .decode()
        .splitlines()
    )
    sec_line = next((sec for sec in section if ".ctors" in sec or ".init_array" in sec), "")
    if not sec_line:
        print("No .ctors/.init_array section found.")
        return

    # determine offset and size of constructor section
    pattern = re.compile(r'\s*\[\s*\d+\]\s*\S+\s*\S+\s*\S+\s*([0-9a-f]+)\s+([0-9a-f]+).*')
    m = pattern.match(sec_line)
    assert m
    off = int(m[1], 16)
    size = int(m[2], 16)
    bytes_per = 8 if isa in ("x86_64", "riscv64") else 4
    print(f"Constructors in {binary} ({hex(off)} : {hex(size)}):")
    if off == 0:
        return

    # extract addresses from this section and determine symbol via nm
    data = run(
        "od", "-t", f"x{bytes_per}", str(binary),
        "-j", hex(off), "-N", hex(size), "-v", f"-w{bytes_per}",
        capture=True
    ).stdout.decode()
    for line in data.splitlines():
        parts = line.split()
        if len(parts) < 2:
            break
        addr = parts[1]
        name = run(str(cross) + "nm", "-C", "-l", str(binary), capture=True).stdout.decode()
        match = next((line for line in name.splitlines() if addr in line), None)
        if match:
            print(match)


@command("list")
def cmd_list(ctx: Context, _: argparse.Namespace) -> None:
    """List the link address of all programs."""
    print("Start of section .text:")
    for entry in ctx.bin_dir.iterdir():
        if entry.is_file() and entry.suffix not in (".o", ".a"):
            cmd = [str(ctx.cross_prefix(entry) + "readelf"), "-S", str(entry)]
            out = run(*cmd, capture=True).stdout.decode()
            for line in out.splitlines():
                if " .text " in line:
                    size = line.split()[4]
                    print(f"{entry.name:>20}: {size}")
                    break


@command(
    "macros",
    [{"name": "path", "help": "path to the Cargo package to expand"}],
)
def cmd_macros(ctx: Context, args: argparse.Namespace) -> None:
    """Expand Rust macros for the given Cargo package."""
    cwd = Path(args.path).resolve()
    if not cwd.is_dir():
        sys.exit(f"{args.path!r} is not a directory.")
    cmd = [
        "cargo",
        "rustc",
        *ctx.rust_target_args,
        "--profile=check",
        "--",
        "-Zunpretty=expanded",
    ]
    paginate(*cmd, cwd=cwd)


@command(
    "straddr",
    [
        {"name": "prog", "help": "binary to search"},
        {"name": "string", "help": "string to look for"},
    ],
)
def cmd_straddr(ctx: Context, args: argparse.Namespace) -> None:
    """Search for `string` inside `prog` and print the absolute address."""
    binary = ctx.bin_dir / args.prog
    if not binary.is_file():
        sys.exit(f"{args.prog!r} not found.")

    print(f"Strings containing '{args.string}' in {binary}:")

    cross = ctx.cross_prefix(binary)
    needle = args.string.encode('utf-8')

    # base address of .rodata
    rodata = run(cross + "readelf", "-S", str(binary), capture=True, text=True).stdout
    base_line = next(line for line in rodata.splitlines() if ".rodata" in line)
    pattern = re.compile(r'\s*\[\s*(\d+)\]\s*\S+\s*\S+\s*([0-9a-f]+).*')
    sec_desc = pattern.match(base_line)
    assert sec_desc
    base = int(sec_desc[2], 16)
    sec_no = int(sec_desc[1])

    # dump the .rodata strings and filter
    out = run(str(cross) + "readelf", "-p", str(sec_no), str(binary), capture=True).stdout
    line_re = re.compile(rb'^\s*\[\s*([0-9A-Fa-f]+)\]\s*(.*)$')
    for line in out.splitlines():
        if needle not in line:
            continue

        m = line_re.match(line)
        if not m:
            continue

        off_hex, rest = m.groups()
        off = int(off_hex, 16)
        sys.stdout.buffer.write((f"0x{base + off:x}: ").encode("ascii") + rest + b"\n")
