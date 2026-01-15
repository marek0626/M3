import argparse
import os
import subprocess
import sys

from collections.abc import Iterator
from pathlib import Path

from .utils import run
from .context import Context
from . import command


@command("checkboot")
def cmd_checkboot(ctx: Context, args: argparse.Namespace) -> None:
    """Validate all boot scripts against the XML schema."""
    errors = 0
    for f in ctx.root.glob("boot/**/*.xml"):
        try:
            run("xmllint", "--schema", "misc/boot.xsd", "--noout", str(f))
        except subprocess.CalledProcessError:
            errors += 1
    if errors:
        sys.exit(f"{errors} boot script(s) failed validation.")


@command(
    "clippy",
    [{"name": "path", "nargs": "?", "help": "path to a Cargo package (relative to repo root)"}],
)
def cmd_clippy(ctx: Context, args: argparse.Namespace) -> None:
    """Run clippy on a specified or every Cargo package."""
    def path_filter(paths: Iterator[Path]) -> list[Path]:
        return [p for p in paths if p != Path("src/Cargo.toml").resolve()]

    if args.path:
        p = Path(args.path).resolve()
        if p.name == "Cargo.toml":
            paths = [p]
        else:
            # Find all Cargo.toml files in the specified directory.
            paths = path_filter(p.glob("**/Cargo.toml"))
    else:
        paths = path_filter(ctx.root.glob("src/**/Cargo.toml"))

    errors = 0
    for cargo in paths:
        # skip vmtest on non‑riscv64
        if ctx.isa != "riscv64" and "vmtest" in str(cargo):
            continue
        # skip rot/raser on non‑riscv
        if not ctx.isa.startswith("riscv") and any(x in str(cargo) for x in ("rot", "raser")):
            continue
        try:
            _run_clippy(ctx, cargo)
        except Exception as exc:
            print(f"clippy failed for {cargo}: {exc}", file=sys.stderr)
            errors += 1
    if errors > 0:
        sys.exit(f"{errors} clippy invocation(s) failed.")


def _run_clippy(ctx: Context, cargo_toml: Path) -> None:
    """Runs clippy on a given Rust crate."""
    # Determine which target / env flags we need.
    target = []
    env = os.environ.copy()

    rel = str(cargo_toml)
    if rel.startswith(str(ctx.root / "tools")):
        target = ctx.rust_host_args
    elif rel.startswith(str(ctx.root / "src/m3lx")):
        env["M3_LX"] = "1"
        target = [
            "--target",
            "riscv64gc-unknown-linux-gnu",
            "--target-dir",
            str(ctx.rust_build),
            "-Z",
            "build-std=core,alloc,std,panic_abort",
        ]
    elif rel.startswith(str(ctx.root / "src/rot")):
        if rel.startswith(str(ctx.root / "src/rot/rots")):
            env["M3_ROTS"] = "1"
        target = [
            "--target",
            "riscv64imc-unknown-none-elf",
            "--target-dir",
            str(ctx.rust_build),
            "-Z",
            "build-std=core,alloc",
        ]
    else:
        target = ctx.rust_target_args

    dir = cargo_toml.parent
    print(f"Running clippy for {dir}...")
    ignore = [
        "clippy::identity_op",
        "clippy::manual_range_contains",
        "clippy::assertions_on_constants",
        "clippy::upper_case_acronyms",
        "clippy::empty_loop",
    ]
    cmd = ["cargo", "clippy"] + target + ["--", "-D", "warnings"]
    for flag in ignore:
        cmd += ["-A", flag]
    run(*cmd, cwd=dir, env=env)


@command("doc")
def cmd_doc(ctx: Context, args: argparse.Namespace) -> None:
    """Generate Rust documentation."""
    os.environ["RUSTDOCFLAGS"] = "-D warnings"
    for lib in (ctx.root / "src/libs/rust").iterdir():
        if lib.is_dir():
            run("cargo", "doc", *ctx.rust_target_args, cwd=lib)
    out = f"file://{ctx.build_dir}/rust/{ctx.rust_target}/doc/m3/index.html"
    print(f"Documentation generated at {out}")


@command(
    "fmt",
    [{"name": "--check", "action": "store_true", "help": "Do not perform changes, just check."}],
)
def cmd_fmt(ctx: Context, args: argparse.Namespace) -> None:
    """
    Run the formatter.

    Additional arguments are passed to `tools/fmt.py`.
    """
    cmd = ["python3", "./tools/fmt.py", *args.remainder]
    if not args.check:
        cmd += ["--inplace"]
    run(*cmd, cwd=ctx.root)


@command(
    "test",
    [{"name": "--coverage", "action": "store_true", "help": "Generate code coverage (w/o miri)."}],
)
def cmd_test(ctx: Context, args: argparse.Namespace) -> None:
    """Run the Rust test suites on host."""
    target = _rust_default_target()
    out_dir = ctx.rust_generic

    if args.coverage:
        os.environ["RUSTFLAGS"] = "-C instrument-coverage=all"
        # clean any previous coverage data
        if (out_dir / "coverage").exists():
            (out_dir / "coverage").rmdir()
        cargo_args = ["test"]
    else:
        cargo_args = ["miri", "test"]

    os.environ["RUST_BACKTRACE"] = "1"
    test_dirs = ("src/libs/rust/thread", "src/libs/rust/base")
    errors = 0

    # run tests
    for dir in test_dirs:
        try:
            # we run in single-threaded mode because some tests work with global data
            run(
                "cargo", *cargo_args, "--target", target, "--target-dir", str(out_dir),
                "--", "--test-threads=1",
                cwd=Path(dir),
            )
        except subprocess.CalledProcessError:
            errors += 1

    if errors:
        sys.exit(f"{errors} test suite(s) failed")

    # generate coverage report
    if args.coverage:
        run(
            "grcov", ".", "-s", ".", "--binary-path", str(out_dir), "-t", "html",
            "--ignore-not-existing", "-o", str(out_dir / "coverage"),
            cwd=ctx.root,
        )
        # delete the *.profraw files that `grcov` created
        for profraw in ctx.root.rglob("*.profraw"):
            profraw.unlink()
        print(f"The coverage results are now available in {out_dir / 'coverage'}")


def _rust_default_target() -> str:
    """Return the default host triple via `rustup`."""
    out = run(
        "rustup", "show",
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, check=False
    ).stdout
    for line in out.splitlines():
        if "Default host:" in line:
            # line looks like: "Default host: x86_64-unknown-linux-gnu"
            return str(line.split()[-1])
    sys.exit("rustup output does not contain a 'Default host' line")


@command(
    "lint",
    [{"name": "lints", "nargs": "?", "help": "The lints to use (default 'async,python')"}]
)
def cmd_lint(ctx: Context, args: argparse.Namespace) -> None:
    """Run the async linter on the kernel, root, and pager."""
    lints = args.lints.split(",") if args.lints else ["async", "python"]

    if "async" in lints:
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(ctx.root / "build/rust")
        dirs = ["src/kernel", "src/libs/rust/resmng", "src/server/root", "src/server/pager"]
        run(
            "python3", "./tools/linter.py", *dirs,
            cwd=ctx.root,
            env=env,
        )

    if "python" in lints:
        pys = [
            ("b",                         {"MYPYPATH": "tools"}),
            ("ci/bootstrap.py",           {}),
            ("ci/builder.py",             {}),
            ("ci/gcdir.py",               {}),
            ("ci/k8s.py",                 {}),
            ("ci/submods-changed.py",     {}),
            ("ci/tests/check_result.py",  {}),
            ("ci/tests/gem5.py",          {}),
            ("ci/web/generate.py",        {"MYPYPATH": "ci/tests"}),
            ("ci/web/publish.py",         {}),
            ("cross/build.py",            {}),
            ("tools/backtrace.py",        {}),
            ("tools/build",               {}),
            ("tools/combine_tculogs.py",  {}),
            ("tools/disk.py",             {}),
            ("tools/execute",             {}),
            ("tools/fmt.py",              {}),
            ("tools/gem5act.py",          {}),
            ("tools/genman.py",           {}),
            ("tools/linter.py",           {}),
            ("tools/syncremote.py",       {}),
        ]
        for py in pys:
            path, add_env = py
            print(f"Running mypy for '{path}'...", flush=True)
            run(
                "mypy", "--python-version", "3.9", "--strict", path,
                env=os.environ.copy() | add_env
            )
