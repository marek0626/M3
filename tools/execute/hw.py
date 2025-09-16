import os
import stat
from pathlib import Path
from typing import List

from base import BasePlatform
from utils import run, which, die


class HWPlatform(BasePlatform):
    """
    The platform for target hw23, and hw.

    This platform runs the configuration on the FPGA attached to $M3_HW_FPGA_HOST.
    """

    def run(self) -> None:
        # ensure required variables are defined
        for var in ("M3_HW_FPGA_HOST", "M3_HW_FPGA_DIR", "M3_HW_FPGA_NO"):
            if not os.getenv(var):
                die(f"environment variable {var} must be defined.")

        # generate config & deps
        self.generate_config()
        self.m3lx.build()

        # determine kernels, modules, and RoT layers
        kernels = self.get_kernels()
        mods = self.get_mods('hw')
        (kernels, mods, rot_layers) = self.add_rot(kernels, mods)

        # collect environment variables
        M3_HW_FPGA_NO = os.getenv("M3_HW_FPGA_NO")
        M3_HW_FPGA_HOST = os.getenv("M3_HW_FPGA_HOST")
        M3_HW_FPGA_DIR = os.getenv("M3_HW_FPGA_DIR")
        M3_HW_RESET = os.getenv("M3_HW_RESET")
        M3_HW_TIMEOUT = os.getenv("M3_HW_TIMEOUT")
        M3_HW_M3LX = os.getenv("M3_HW_M3LX")
        M3_HW_TTY = os.getenv("M3_HW_TTY")
        M3_HW_VM = os.getenv("M3_HW_VM")
        M3_HW_PAUSE = os.getenv("M3_HW_PAUSE")

        # build arguments for fpga script
        if self.target == "hw23":
            args = "--version 2"
        else:
            args = "--version 4"
        args += f" --fpga {M3_HW_FPGA_NO}"
        args += f" --logflags {self.logflags}"
        if M3_HW_RESET == "1":
            args += " --reset"
        if M3_HW_TIMEOUT:
            args += f" --timeout={M3_HW_TIMEOUT}"
        if M3_HW_VM != "0":
            args += " --vm"
        if M3_HW_M3LX:
            if not M3_HW_TTY:
                die("Please define M3_HW_TTY first.")
            args += f" --serial {M3_HW_TTY}"

        # collect files to transfer
        files = [str(self.outdir / "boot.xml")]
        if rot_layers:
            first, *rest = rot_layers.split(",")
            args += f" --tile '{Path(first).name}' --rotlayer '{Path(first).name}'"
            files.append(first)
            for layer in rest:
                args += f" --rotlayer '{Path(layer).name}'"
                files.append(layer)
        else:
            for k in kernels.split(","):
                args += f" --tile '{Path(k).name}'"
                files.append(k.split()[0])
        for mod in mods.split(","):
            args += f" --mod '{mod}'"
            files.append(mod.split("=", 1)[1])

        run_sh = self._generate_run_script(args, str(M3_HW_FPGA_DIR), str(M3_HW_PAUSE))

        # copy everything to the remote FPGA host
        remote = f"{M3_HW_FPGA_HOST}:{M3_HW_FPGA_DIR}"
        rsync_cmd = [
            which("rsync"), "-rz",
            "tools/fpga", "platform/hw/fpga_tools/python", *files, str(run_sh),
            remote,
        ]
        run(*rsync_cmd)

        # run the configuration on FPGA
        ssh_cmd = ["ssh", "-t", str(M3_HW_FPGA_HOST), f"cd {M3_HW_FPGA_DIR} && sh run.sh"]
        run(*ssh_cmd)

        # pull back the log files
        scp_cmd = [
            "scp", "-q",
            f"{M3_HW_FPGA_HOST}:{M3_HW_FPGA_DIR}/log.txt",
            f"{M3_HW_FPGA_HOST}:{M3_HW_FPGA_DIR}/log/pm*",
            str(self.outdir),
        ]
        run(*scp_cmd)

    def _generate_run_script(self, args: str, remote_dir: str, pause: str) -> Path:
        """Generates the run.sh to be executed on the FPGA-hosting machine."""
        run_sh = self.outdir / "run.sh"
        with run_sh.open("w") as f:
            f.write("#!/bin/sh\n")
            f.write(f"export PYTHONPATH=$HOME/{remote_dir}/python:$PYTHONPATH\n")
            f.write("\n")
            if self.debug:
                f.write('echo -n > .running\n')
                f.write('trap "rm -f .running 2>/dev/null" SIGINT SIGTERM EXIT\n')
                f.write('rm -f .ready\n')
                f.write(f"python3 ./fpga/main.py {args} --debug {pause} &>log.txt &\n")
                f.write('fpga=$!\n')
                f.write('echo "Waiting until FPGA has been initialized..."\n')
                f.write('while [ "`cat .ready 2>/dev/null`" = "" ] &&\n')
                f.write('      [ -f /proc/$fpga/cmdline ]; do\n')
                f.write('  sleep 1\n')
                f.write('done\n')
                f.write('[ -f /proc/$fpga/cmdline ] || { cat log.txt && exit 1; }\n')
                f.write('trap "trap - SIGTERM && kill -- -$$" SIGINT SIGTERM EXIT\n')
                f.write('OPENOCD=$HOME/tcu/fpga_tools/debug\n')
                f.write('$OPENOCD/openocd -f $OPENOCD/fpga_switch.cfg >openocd.log 2>&1\n')
            else:
                f.write(f"python3 ./fpga/main.py {args} 2>&1 | tee -i log.txt\n")
        run_sh.chmod(run_sh.stat().st_mode | stat.S_IXUSR)
        return run_sh
