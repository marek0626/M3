import os
import re
import shutil
import subprocess
import sys
import tempfile

from pathlib import Path
from m3lx import M3Lx
from typing import Tuple
from utils import die, run, which, xml_xpath, xml_attr_value


class BasePlatform:
    """The base class for all platforms."""

    def __init__(self, cfg: Path, crossname: str, debug: bool):
        """
        Constructs the base platform.

        The argument `cfg` denotes the path to the configuration file to start, `crossname` is the
        name of the cross-compiler toolchain (e.g., "riscv64-buildroot-linux-musl-"), and `debug`
        specifies whether we are debugging.
        """
        self.cfg = cfg.resolve()
        self.crossname = crossname
        self.debug = debug

        # basic env vars
        self.target = str(os.getenv("M3_TARGET"))
        self.isa = str(os.getenv("M3_ISA"))
        self.build = str(os.getenv("M3_BUILD"))
        self.logflags = os.getenv("M3_LOG", "Info,Error")

        # directory paths
        self.builddir = Path("build") / f"{self.target}-{self.isa}-{self.build}"
        self.crossdir = Path("build") / f"cross-{self.isa}" / "host/bin"
        if self.build == "debug":
            self.bindir = self.builddir / "bin"
        else:
            self.bindir = self.builddir / "bin/stripped"
        self.outdir = Path(os.getenv("M3_OUT", "run"))
        self.outdir.mkdir(parents=True, exist_ok=True)

        # m3lx and module directory
        self.m3lx = M3Lx(self)
        defmoddir = Path(os.getenv("M3_MOD_PATH", str(self.builddir)))
        if self.m3lx.enabled:
            # use temporary module path (parallel‑run safety)
            self.moddir = Path(tempfile.mkdtemp())
            for item in defmoddir.iterdir():
                if item.is_file():
                    shutil.copy(item, self.moddir / item.name)
        else:
            self.moddir = defmoddir

    def __del__(self) -> None:
        if self.m3lx.enabled:
            shutil.rmtree(self.moddir, ignore_errors=True)

    # Abstract entry point – must be implemented by subclasses
    def run(self) -> None:
        raise NotImplementedError("Sub‑class must implement run()")

    def generate_config(self) -> None:
        """Generate self.outdir/boot.xml from the configuration file."""
        # validate against XSD
        run(which("xmllint"), "--schema", "misc/boot.xsd", "--noout", str(self.cfg))

        # export <env> entries
        env_txt = xml_xpath(self.cfg, "/config/env/text()")
        for entry in env_txt.split():
            var, _, val = entry.partition("=")
            self._env_export(var, val)

        # write optional <app> element to boot.xml (no app is fine for cases like standalone.xml)
        app_xml = xml_xpath(self.cfg, "/config/dom/app")
        (self.outdir / "boot.xml").write_text(app_xml)

    @staticmethod
    def _env_export(var: str, val: str) -> None:
        """Export a variable only if it is not already set."""
        if var in os.environ:
            if os.environ[var] != val:
                print(
                    f"Warning: {var} is already set to '{os.environ[var]}',"
                    f" ignoring overwrite to '{val}' by config.",
                    file=sys.stderr,
                )
        else:
            os.environ[var] = val

    def get_kernels(self) -> str:
        """Return a comma‑separated list of kernel binaries (with args)."""
        kernel_tags = xml_xpath(self.cfg, "//kernel/@args")
        if not kernel_tags:
            return ""

        kernels = []
        for line in kernel_tags.split("\n"):
            args = xml_attr_value(line)
            kernels.append(f"{self.bindir}/{args}")
        return ",".join(kernels)

    def get_mods(self, mode: str) -> str:
        """Return a comma-separated list of modules."""
        parts = [f"boot.xml={self.outdir}/boot.xml"]

        # modules referenced by <app args="…">
        app_args = xml_xpath(self.cfg, ".//app[@args]/@args")
        for arg in app_args.split('\n'):
            arg_val = xml_attr_value(arg)
            if not arg_val:
                continue
            # we currently assume that binaries starting with "/" are loaded from the FS
            if arg_val.startswith("/"):
                continue
            name = arg_val.split(" ")[0]
            if mode != "hw" and name == "disk" and not os.getenv("M3_GEM5_HDD"):
                die("Please specify the HDD image to use via M3_GEM5_HDD.")
            path = self.bindir / name
            if not path.is_file():
                die(f"Binary '{path}' does not exist.")
            parts.append(f"{name}={path}")

        # modules defined under <mods>
        mods_xml = xml_xpath(self.cfg, "/config/mods/mod")
        for m in re.finditer(r'<mod\s+name="([^"]+)"\s+file="([^"]+)"', mods_xml):
            name, filename = m.groups()
            src = self.moddir / filename
            if not src.is_file():
                die(f"Boot module '{src}' does not exist.")
            parts.append(f"{name}={src}")

        # we always need tilemux
        parts.append(f"tilemux={self.bindir}/tilemux")

        return ",".join(parts)

    def add_rot(self, kernels: str, mods: str) -> Tuple[str, str, str]:
        """
        Returns the kernels, modules, and RoT layers.

        If there are RoT layers, the kernels and modules are changes accordingly and the third
        return value is a comma-separated list of RoT layers. Otherwise this list is empty and
        kernels and modules are not changed.
        """
        layers = xml_xpath(self.cfg, "string(/config/rot/@layers)")
        if not layers:
            return (kernels, mods, "")

        out = []
        for name in layers.split(","):
            p = self.builddir / "rotbin" / name
            if not p.is_file():
                die(f"RoT layer '{p}' does not exist.")
            out.append(str(p))

        # extract the single kernel (multiple not support with RoT)
        kernel = kernels.split(",")[0]
        kernels = f"{Path(kernel).name}"
        # ensure the kernel binary exists
        kernel = kernel.split(" ")[0]
        if not Path(kernel).is_file():
            die(f"Kernel '{kernel}' does not exist.")
        mods = f"{mods},kernel={kernel}"
        rot_layers = ",".join(out)

        self._print_module_hashes(f"{rot_layers},{mods}")

        return (kernels, mods, rot_layers)

    def _print_module_hashes(self, modules: str) -> None:
        """Prints the SHA3-224 hashes of `modules`, given as a comma-separated list."""
        if not which("openssl"):
            print("NOTE: openssl is not installed. Skipping hashes of boot modules.")
            return

        for mod in modules.split(","):
            if "=" in mod:
                name, path = mod.split("=", 1)
            else:
                name, path = "RoT layer", mod
            h = run(
                which("openssl"), "dgst", "-sha3-224", path,
                capture=subprocess.PIPE,
            ).stdout.split()[-1]
            print(f"SHA3-224 hash of {name} ({path}): {h}")
