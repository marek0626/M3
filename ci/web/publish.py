#!/usr/bin/env python3

import argparse
import sys
import subprocess
from pathlib import Path

NAMESPACE = "os"
POD_NAME = "m3-ci-web-0"
REMOTE_WEB_PATH = "/web"


def clear_remote_dir() -> None:
    subprocess.run([
        "kubectl", "exec", "-n", NAMESPACE, "-t", POD_NAME,
        "--",
        "sh", "-c", f"rm -rf {REMOTE_WEB_PATH}/*",
    ], check=True)


def copy_local_to_remote(local_dir: Path) -> None:
    dest = f"{POD_NAME}:{REMOTE_WEB_PATH}"
    for entry in sorted(local_dir.iterdir()):
        subprocess.run([
            "kubectl", "cp", "-n", NAMESPACE, str(entry), dest,
        ], check=True)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Copy a local directory into the /web directory of the pod m3-ci-web-0."
    )
    parser.add_argument("directory", type=Path, help="local directory to upload")
    args = parser.parse_args(sys.argv[1:])

    if not args.directory.is_dir():
        parser.error(f'"{args.directory}" is not a directory or does not exist.')

    try:
        clear_remote_dir()
        copy_local_to_remote(args.directory)
    except subprocess.CalledProcessError as exc:
        print(f"Error: command {' '.join(exc.cmd)} exited with status {exc.returncode}",
              file=sys.stderr)
        return exc.returncode

    return 0


if __name__ == "__main__":
    sys.exit(main())
