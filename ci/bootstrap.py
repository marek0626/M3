#!/usr/bin/env python3

import argparse
import os
import subprocess
import sys
from pathlib import Path


def run(*cmd: str, cwd: Path | None = None):
    subprocess.run(cmd, cwd=str(cwd) if cwd else None, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run CI bootstrap on specific M³ commit.")
    parser.add_argument("commit", help="git commit to check out")
    parser.add_argument("--no-build", action="store_true", help="skip the build step")
    args = parser.parse_args()

    # clone repo, if necessary
    repo_dir = Path("M3")
    if not repo_dir.is_dir():
        home = Path.home()
        user = (home / ".gitlab" / "user").read_text().strip()
        pw = (home / ".gitlab" / "pw").read_text().strip()
        repo = f"https://{user}:{pw}@gitlab.barkhauseninstitut.org/os/code/M3/M3.git"
        run("git", "clone", repo)

    # checkout commit
    os.chdir(repo_dir)
    run("git", "checkout", args.commit)

    # perform bootstrap
    run(sys.executable, "./ci/builder.py", "prepare")
    if not args.no_build:
        run(
            "nix", "develop", "path:nix", "-c",
            sys.executable, "./ci/builder.py", "build",
            "--build", "debug", "bench"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
