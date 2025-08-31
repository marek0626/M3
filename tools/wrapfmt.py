#!/usr/bin/env python3

# Script for wrapping a formatting program that does not support in-place updates and dry-runs.
#
# Author: Viktor Reusch

import argparse
import subprocess
import os
import sys
import tempfile

from difflib import unified_diff
from typing import List


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Script for wrapping a formatting program that does not support in-place updates and dry-runs",
        epilog="The formatter command line will be split at spaces. The file path is appended to it.",
    )
    parser.add_argument(
        "-i",
        "--inplace",
        action="store_true",
        help="replace file with formatted version",
    )
    parser.add_argument(
        "cmd",
        help="formatter command line",
    )
    parser.add_argument(
        "path",
        help="path to the file to format",
    )
    args = parser.parse_args()

    cmd = args.cmd.split()
    path = args.path
    parent = os.path.dirname(path)
    basename = os.path.basename(path)

    # Execute formatter program and capture formatted output.
    full_cmd = cmd + [path]
    result = subprocess.run(full_cmd, stdout=subprocess.PIPE)
    if result.returncode != 0:
        cmd_str = " ".join(full_cmd)
        print(f'execution of "{cmd_str}" failed', file=sys.stderr)
        exit(result.returncode)

    # Read original version of the file contents.
    with open(path, "rb") as file:
        contents = file.read()

    # Abort if already formatted.
    if contents == result.stdout:
        return
    print(f"{path} needs formatting", file=sys.stderr)

    if args.inplace:
        # Replace with formatted version in a crash-consistent way.
        with tempfile.NamedTemporaryFile(
            dir=parent, prefix=basename + ".", suffix=".format", delete=False
        ) as formatted:
            formatted.write(result.stdout)
        os.replace(formatted.name, path)
    else:
        # Print diff and exit with error.
        diff = unified_diff(to_lines(contents), to_lines(result.stdout))
        sys.stdout.writelines(diff)
        exit(1)


def to_lines(b: bytes) -> List[str]:
    return b.decode("UTF-8").splitlines(keepends=True)


if __name__ == "__main__":
    main()
