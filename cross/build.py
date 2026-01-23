#!/usr/bin/env python3

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import List, NoReturn, Tuple


def exec_replace(argv: List[str]) -> NoReturn:
    os.execvp(argv[0], argv)


def parse_args(argv: List[str]) -> Tuple[argparse.Namespace, List[str]]:
    parser = argparse.ArgumentParser(description="Build Buildroot cross toolchains")
    parser.add_argument(
        "arch",
        choices=("x86_64", "riscv64", "riscv32"),
        help="Target architecture",
    )
    parser.add_argument(
        "--jobs", "-j",
        help="Number of concurrent jobs (CPU count by default)",
    )
    return parser.parse_known_args(argv)


def check_configs(dist: Path, config: Path, config_origin: Path, arch_config: Path) -> None:
    try:
        result = subprocess.run(
            ["cmp", str(config_origin), str(arch_config)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        same = result.returncode == 0
    except FileNotFoundError:
        same = True  # If cmp is unavailable, suppress warning

    if not same:
        print(f"\033[1mWARNING: {config_origin} and {arch_config} differ\033[0m")
        print(f"This probably indicates that {arch_config} was updated and you should rebuild.")
        choice = input(
            "Do you want to rebuild completely (r), update to the new config (u), "
            f"or continue with the potentially outdated {config} (c)? "
        ).strip()

        if choice == "r":
            shutil.rmtree(dist, ignore_errors=True)
        elif choice == "u":
            if config.exists():
                config.unlink()
        elif choice == "c":
            pass
        else:
            sys.exit(1)


def create_config(root: Path, dist: Path, arch: str) -> None:
    config = dist / ".config"
    config_origin = dist / ".config-origin"
    arch_config = root / f"config-{arch}"

    # detect config mismatch
    if config.exists() and config_origin.exists():
        check_configs(dist, config, config_origin, arch_config)

    # initial defconfig
    if not config.exists():
        subprocess.check_call(
            [
                "make",
                f"O={dist}",
                "defconfig",
                f"BR2_DEFCONFIG={arch_config}",
            ],
            cwd=root / "buildroot",
        )
        dist.mkdir(parents=True, exist_ok=True)
        shutil.copy2(arch_config, config_origin)


def main(argv: List[str]) -> None:
    # buildroot builds its own Python and gets confused if PYTHONPATH is set
    os.environ.pop("PYTHONPATH", None)

    # enter nix-provided FHS environment if /usr/bin/file is missing
    file_path = Path("/usr/bin/file")
    if not (file_path.is_file() and os.access(file_path, os.X_OK)):
        exec_replace(["m3-fhs-env", *sys.argv])

    args, remainder = parse_args(argv)

    # check and potentially create config
    script_path = Path(sys.argv[0]).resolve()
    root = script_path.parent
    dist = (root / "..").resolve() / "build" / f"cross-{args.arch}"
    create_config(root, dist, args.arch)

    # build
    jobs = args.jobs if args.jobs else (os.cpu_count() or 1)
    make_jobs = f"-j{jobs}"
    subprocess.check_call(
        ["make", f"O={dist}", make_jobs, *remainder],
        cwd=root / "buildroot",
    )


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except KeyboardInterrupt:
        pass
