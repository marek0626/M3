import os
import tempfile

from pathlib import Path
from typing import List
from base import BasePlatform
from utils import die, run


class Gem5Platform(BasePlatform):
    """
    The platform for target gem5.

    This platform runs the configuration on the gem5 simulator.
    """

    def run(self):
        # generate config & deps
        self.generate_config()
        self.m3lx.build()

        # determine kernels, modules, and RoT layers
        kernels = self.get_kernels()
        mods = self.get_mods('gem5')
        (kernels, mods, rot_layers) = self.add_rot(kernels, mods)

        # collect environment variables (with defaults)
        M3_GEM5_CORES = int(os.getenv("M3_GEM5_CORES", "16"))
        M3_GEM5_CFG = os.getenv("M3_GEM5_CFG", "platform/gem5/configs/m3/default.py")
        M3_GEM5_CPU = os.getenv("M3_GEM5_CPU", "TimingSimpleCPU" if self.debug else "DerivO3CPU")
        M3_GEM5_CPUFREQ = os.getenv("M3_GEM5_CPUFREQ", "1GHz")
        M3_GEM5_MEMFREQ = os.getenv("M3_GEM5_MEMFREQ", "333MHz")
        M3_GEM5_PAUSE = os.getenv("M3_GEM5_PAUSE")
        M3_GEM5_LOG = os.getenv("M3_GEM5_LOG", "Tcu")
        M3_GEM5_LOGSTART = os.getenv("M3_GEM5_LOGSTART")
        M3_GEM5_HDD = os.getenv("M3_GEM5_HDD")
        DBG_GEM5 = os.getenv("DBG_GEM5")

        # Pad kernel list so that we have exactly <cores> entries
        kernel_list = kernels.split(",")
        while len(kernel_list) < M3_GEM5_CORES:
            kernel_list.append("")
        kernels = ",".join(kernel_list)

        # build the gem5 argument list
        params: List[str] = [
            f"--outdir={self.outdir}",
            "--debug-file=gem5.log",
            f"--debug-flags={M3_GEM5_LOG}",
        ]
        if M3_GEM5_PAUSE:
            params.append("--listener-mode=on")
        if M3_GEM5_LOGSTART:
            params.append(f"--debug-start={M3_GEM5_LOGSTART}")
        params.extend(
            [
                M3_GEM5_CFG,
                "--cpu-type", M3_GEM5_CPU,
                "--isa", self.isa,
                "--cmd", kernels,
                "--mods", mods,
                "--logflags", self.logflags
            ]
        )
        if rot_layers:
            params.append(f"--rot-layers={rot_layers}")
        params.append(f"--cpu-clock={M3_GEM5_CPUFREQ}")
        params.append(f"--sys-clock={M3_GEM5_MEMFREQ}")
        if M3_GEM5_PAUSE:
            params.append(f"--pausetile={M3_GEM5_PAUSE}")

        # build environment for gem5
        env = os.environ.copy()
        if M3_GEM5_HDD and not Path(M3_GEM5_HDD).is_file():
            die(f"Hard disk image '{M3_GEM5_HDD}' does not exist.")
        env["M3_GEM5_IDE_DRIVE"] = str(M3_GEM5_HDD) if M3_GEM5_HDD else ""
        env["M3_GEM5_TILES"] = str(M3_GEM5_CORES)
        env["M5_PATH"] = self.builddir

        # start gem5
        build_dir = Path("build/gem5")
        gem5_build = "X86" if self.isa == "x86_64" else "RISCV"
        if DBG_GEM5:
            # tiny gdb helper – create a temporary command file
            with tempfile.NamedTemporaryFile("w", delete=False) as tf:
                tf.write("b main\nrun " + " ".join(params) + "\n")
                cmd_file = Path(tf.name)
            try:
                # run gem5 in gdb
                run(
                    "gdb",
                    "--tui",
                    str(build_dir / f"build/{gem5_build}/gem5.debug"),
                    f"--command={str(cmd_file)}",
                    env=env,
                )
            finally:
                # delete temp file
                try:
                    cmd_file.unlink()
                except FileNotFoundError:
                    pass
        else:
            # run gem5
            exe = build_dir / f"build/{gem5_build}/gem5.opt"
            if self.debug:
                wrapper = self.builddir / "toolsbin/ignoreint"
                run(str(wrapper), str(exe), *params, env=env)
            else:
                run(str(exe), *params, env=env)
