#!/usr/bin/env python3

import argparse
import os
import sys
import subprocess


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Helper script for linting multiple crates using the m3-lints crate"
    )
    parser.add_argument(
        "crates",
        metavar="CRATE",
        nargs="+",
        help="paths to crates that should be checked",
    )
    args = parser.parse_args()

    lints_path = os.path.join(os.path.abspath(os.path.dirname(sys.argv[0])), "lints")

    subprocess.run(
        ("cargo", "install", "--locked", "cargo-dylint@4.1.0", "dylint-link@4.1.0"), check=True
    )

    # Dylint fails if the driver path is overridden but nonexistent.
    driver_path = os.environ.get("DYLINT_DRIVER_PATH")
    if driver_path is not None:
        os.makedirs(driver_path, exist_ok=True)

    for crate in args.crates:
        subprocess.run(
            (
                "cargo",
                "dylint",
                f"--path={lints_path}",
                "--",
                "-Z",
                "build-std=core,alloc,std,panic_abort",
            ),
            cwd=crate,
            check=True,
        )


if __name__ == "__main__":
    main()
