import re
import shutil
import subprocess
import sys

from pathlib import Path
from typing import Optional


def run(*cmd: str,
        cwd: Path | None = None,
        capture: int | None = None,
        check: bool = True,
        env: Optional[dict] = None) -> subprocess.CompletedProcess:
    """Thin wrapper around subprocess.run that prints an error on failure."""
    try:
        return subprocess.run(
            cmd,
            cwd=str(cwd) if cwd else None,
            text=True,
            check=check,
            env=env,
            stdout=capture,
            stderr=capture,
        )
    except subprocess.CalledProcessError as e:
        print(f"Command {' '.join(cmd)} failed (exit {e.returncode})", file=sys.stderr)
        sys.exit(e.returncode)


def which(name: str) -> str:
    """Return absolute path of an executable or abort."""
    path = shutil.which(name)
    if not path:
        die(f"Required program '{name}' not found in $PATH")
    return path


def die(msg: str):
    """Print an error and exit with a non‑zero status."""
    print(msg, file=sys.stderr)
    sys.exit(1)


def parse_size(size: str) -> int:
    """Parses the given integer as a size supporting K, M, and G suffixes."""
    unit = size[-1].upper()
    num = int(size[:-1])
    if unit == "G":
        res = num * 1024 ** 3
    elif unit == "M":
        res = num * 1024 ** 2
    elif unit == "K":
        res = num * 1024
    else:
        res = int(size)
    return res


def xml_xpath(file: Path, xpath: str) -> str:
    """Extracts parts of given XML file using the given XPath expression."""
    return run(
        which("xmllint"),
        "--xpath",
        xpath,
        str(file),
        capture=subprocess.PIPE,
        check=False,
    ).stdout.strip()


def xml_attr_value(xml: str) -> str | None:
    """Extracts the attribute value from XML such as ' attr="value"'."""
    pattern = re.compile(r'\S+="(.*?)"')
    m = pattern.match(xml.strip())
    if m:
        return m.group(1)
    else:
        return None
