#!/usr/bin/env python3

import argparse
import os
import sys

from pathlib import Path

from base import BasePlatform
from gem5 import Gem5Platform
from hw import HWPlatform
from utils import die, run


def main() -> None:
    parser = argparse.ArgumentParser(description="M³ executor (gem5 & HW).")
    parser.add_argument("crossname", help="Name of the cross‑toolchain")
    parser.add_argument("script", help="Path to the XML configuration file")
    parser.add_argument("--debug", action="store_true", help="Whether debugging mode is used")
    args = parser.parse_args()

    cfg_path = os.path.abspath(args.script)
    if not os.path.isfile(cfg_path):
        die(f"Configuration file '{cfg_path}' does not exist.")

    # choose the platform based on $M3_TARGET (or $M3_RUN_GEM5)
    target = os.getenv("M3_TARGET", "gem5")
    if target == "gem5" or os.getenv("M3_RUN_GEM5") == "1":
        platform: BasePlatform = Gem5Platform(Path(cfg_path), args.crossname, args.debug)
    elif target in {"hw", "hw23"}:
        platform = HWPlatform(Path(cfg_path), args.crossname, args.debug)
    else:
        die(f"Unknown target '{target}'")

    # Run the configuration on the selected platform
    platform.run()

    # Restore terminal to cooked mode
    if sys.stdin.isatty():
        run("stty", "sane")


if __name__ == "__main__":
    main()
