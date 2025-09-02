import os
import subprocess
import sys

from pathlib import Path
from typing import Any, Mapping, Optional


def popen(*cmd: str,
          cwd: Optional[Path] = None,
          env: Optional[Mapping[str, str]] = None,
          stdin: Optional[int] = None,
          stdout: Optional[int] = None,
          stderr: Optional[int] = None,
          text: bool = False) -> subprocess.Popen[Any]:
    """Wrapper around subprocess.Popen that prints the command when M3_VERBOSE=1."""
    if os.getenv("M3_VERBOSE") == "1":
        print(">>>", " ".join(cmd), file=sys.stderr)
    return subprocess.Popen(
        cmd,
        cwd=str(cwd) if cwd else None,
        env=env,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        text=text,
    )


def run(*cmd: str,
        check: bool = True,
        cwd: Optional[Path] = None,
        env: Optional[Mapping[str, str]] = None,
        capture: bool = False,
        stdin: Optional[int] = None,
        stdout: Optional[int] = None,
        stderr: Optional[int] = None,
        text: bool = False) -> subprocess.CompletedProcess[Any]:
    """Wrapper around subprocess.run that prints the command when M3_VERBOSE=1."""
    if os.getenv("M3_VERBOSE") == "1":
        print(">>>", " ".join(cmd), file=sys.stderr)
    return subprocess.run(
        cmd,
        check=check,
        cwd=str(cwd) if cwd else None,
        env=env,
        capture_output=capture,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        text=text,
    )


def run_and_tee(*cmd: str,
                check: bool = True,
                cwd: Optional[Path] = None,
                env: Optional[Mapping[str, str]] = None,
                log_file: Path) -> None:
    """Execute command and print its stdout+stderr to stdout and given log file."""
    if os.getenv("M3_VERBOSE") == "1":
        print(">>>", " ".join(cmd), file=sys.stderr)
    with log_file.open("wb") as log_f:
        proc = subprocess.Popen(
            args=list(map(str, cmd)),
            env=env,
            cwd=str(cwd) if cwd else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if proc.stdout is None:
            raise RuntimeError("Pipe creation failed")

        # Read line‑by‑line, write to both destinations.
        for raw_line in proc.stdout:
            # raw_line already contains the trailing newline.
            log_f.write(raw_line)
            log_f.flush()
            sys.stdout.buffer.write(raw_line)
            sys.stdout.buffer.flush()

        proc.wait()
        if check and proc.returncode != 0:
            sys.exit(f"{' '.join(cmd)} exited with status {proc.returncode}")


def paginate(*cmd: str,
             cwd: Optional[Path] = None,
             stdin: Optional[int] = None) -> None:
    """Run *cmd* and pipe through `less` when stdout is a TTY."""
    if os.getenv("M3_VERBOSE") == "1":
        print(">>>", " ".join(cmd), file=sys.stderr)
    if sys.stdout.isatty():
        prod = subprocess.Popen(
            args=list(map(str, cmd)),
            cwd=str(cwd) if cwd else None,
            stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE
        )
        if prod.stdout is None or prod.stderr is None:
            raise RuntimeError("Pipe creation failed")
        pager = subprocess.Popen(["less"], stdin=prod.stdout)
        # give the pipe to less only
        prod.stdout.close()
        for line in prod.stderr:
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        pager.wait()
        prod.wait()
    else:
        run(*cmd)
