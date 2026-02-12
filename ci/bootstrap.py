#!/usr/bin/env python3

import argparse
import os
import subprocess
import sys

from pathlib import Path
from typing import Optional


def run(*cmd: str, cwd: Optional[Path] = None) -> None:
    subprocess.run(cmd, cwd=str(cwd) if cwd else None, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run CI bootstrap on specific M³ commit.")
    parser.add_argument("commit", help="git commit to check out")
    parser.add_argument("--no-build", action="store_true", help="skip the build step")
    args = parser.parse_args()
    home = Path.home()
    pw = (home / ".gitlab" / "pw").read_text().strip()

    # use submodules from gitlab, not github
    (home / ".gitconfig").write_text(
        rf'[url "https://m3-ci:{pw}@gitlab.barkhauseninstitut.org/os/code/M3/"]'
        "\n\t"
        r'insteadOf = https://github.com/Barkhausen-Institut/'
    )

    # clone repo, if necessary
    repo_dir = Path("M3")
    if not repo_dir.is_dir():
        repo = f"https://m3-ci:{pw}@gitlab.barkhauseninstitut.org/os/code/M3/M3.git"
        run("git", "clone", repo)

    # checkout commit
    os.chdir(repo_dir)
    run("git", "checkout", args.commit)

    # perform bootstrap
    run(sys.executable, "./ci/builder.py", "prepare", "--m3lx")
    if not args.no_build:
        run(
            "nix", "develop", "path:nix", "-c",
            sys.executable, "./ci/builder.py", "build", "--build", "debug", "bench", "--m3lx"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
