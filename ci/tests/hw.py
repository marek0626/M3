#!/usr/bin/env python3

import argparse
import subprocess
import sys
import traceback

from pathlib import Path
from typing import Dict

from base import Test, Runner, FSImages

MAX_RETRIES = 3

parser = argparse.ArgumentParser(description='This is the hardware test runner.')
parser.add_argument('--tests', nargs='+', default=[], help='the tests to run')
parser.add_argument('--builds', nargs='+', default=['debug', 'bench'],
                    help='the build modes to use')
parser.add_argument('--targets', nargs='+', default=['hw23', 'hw'],
                    help='the targets to run the tests on')
parser.add_argument('--types', nargs='+', default=['a', 'b', 'sh'],
                    help='the tile types to run the tests on '
                         '(a=SPM, b=Caches+VM, sh=Caches+VM+Sharing)')
parser.add_argument('results', help='The folder to use for the test results.')
args = parser.parse_args()

fpga_vars = ["M3_HW_FPGA_HOST", "M3_HW_FPGA_DIR", "M3_HW_FPGA_NO", "M3_HW_VIVADO"]
for v in fpga_vars:
    if os.getenv(v) is None:
        sys.exit(f"Please define {v} first")

all_tests = [
    # "lxrust-benchs", "lxcpp-benchs", "lxtcutest",
    "rust-net-tests-lo", "cpp-net-tests-lo", "rust-net-benchs-lo", "cpp-net-benchs-lo",
    "rust-algo-tests", "rust-destr-tests", "rust-misc-tests", "rust-vfs-tests",
    "rust-algo-benchs", "rust-misc-benchs", "rust-vfs-benchs",
    "cpp-algo-benchs", "cpp-misc-benchs", "cpp-vfs-benchs",
    "chantests", "unittests", "resmngtest",
    "rots-hello", "rots-evidence-test", "hashmux-tests", "bench-hashfile-tee", "tee-msgchan",
    "hello",
    "find", "tar", "untar", "sqlite", "leveldb", "sha256sum", "sort",
    "cat_awk", "cat_wc", "grep_awk", "grep_wc",
    "facever", "voiceassist-udp", "voiceassist-tcp",
    "ycsb-bench-udp", "ycsb-bench-tcp",
    "standalone", "msgchan", "rust-sndrcv", "vmtest",
    "bench-shell", "shell-nested", "parchksum", "filterchain",
    "libctest", "rust-std-test",
]

if len(args.tests) == 0:
    args.tests = all_tests


class HWTest(Test):
    def should_run(self) -> bool:
        # hw23 does not have a RoT
        if self.target == "hw23" and self.is_rot_test():
            return False
        return super().should_run()

    def build_env(self, rundir: Path) -> Dict[str, str]:
        vars = super().build_env(rundir)
        vars["M3_HW_RESET"] = "1"
        vars["M3_HW_TIMEOUT"] = "60"
        return vars


class HWRunner(Runner):
    def _finish_test(self, test: Test, force: bool) -> bool:
        if not force and not self._system_started(test) and test.retries < MAX_RETRIES:
            self._reload_fpga(test)
            self._print_progress(f"Repeating {test.run_dirname()}")
            test.reset()
            return False
        else:
            return super()._finish_test(test, force)

    def _system_started(self, test: Test) -> bool:
        with open(test.log_file(self.dir), "r") as file:
            for line in file:
                if "Kernel is ready" in line:
                    return True
        return False

    def _reload_fpga(self, test: Test):
        if test.target == "hw23":
            bitfile = "fpga_top_v4.6.0.bit"
        else:
            bitfile = "fpga_top_v4.10.7.bit"
        # try that multiple times as it fails sometimes
        for i in range(3):
            self._print_progress(f"Loading bitfile {bitfile}")
            try:
                subprocess.run(
                    ["./b", "loadfpga", bitfile],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=True
                )
                break
            except subprocess.CalledProcessError:
                pass
        else:
            raise RuntimeError("Unable to load bitfile")


# create FS images
for target in args.targets:
    images = FSImages(target)
    for build in args.builds:
        images.build("riscv64", build, 64, "default", 32 * 1024, 512)
        images.build("riscv64", build, 64, "bench", 48 * 1024, 4096)

# collect jobs
runner = HWRunner(Path(args.results))
for test in args.tests:
    for build in args.builds:
        for target in args.targets:
            for ty in args.types:
                t = HWTest(test, target, build, "riscv64", ty, 64)
                if t.should_run():
                    runner.add(t)

# execute everything
try:
    runner.run(1)
except Exception:
    print(traceback.format_exc())
    print("Stopping tests...")
    runner.stop()
except KeyboardInterrupt:
    print("Stopping tests...", flush=True)
    runner.stop()

# print summary and exit with 0/1
res = runner.summary()
sys.exit(res)
