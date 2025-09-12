#!/usr/bin/env python3

import argparse
import os
import resource
import shlex
import shutil
import subprocess
import sys
import traceback

from pathlib import Path
from typing import Dict

from base import Test, Runner, FSImages, indir

parser = argparse.ArgumentParser(description='This is the gem5 test runner.')
parser.add_argument('--tests', nargs='+', default=[], help='the tests to run')
parser.add_argument('--isas', nargs='+', default=['riscv32', 'riscv64', 'x86_64'],
                    help='the ISAs to run the tests with (riscv32, riscv64, x86_64)')
parser.add_argument('--types', nargs='+', default=['a', 'b', 'sh'],
                    help='the tile types to run the tests on '
                         '(a=SPM, b=Caches+VM, sh=Caches+VM+Sharing)')
parser.add_argument('--bpes', nargs='+', type=int, default=[32, 64],
                    help='the blocks-per-extent values to run the tests with. '
                         'Note that this also selects the CPU model (64=OoO, 32=Timing).')
parser.add_argument('--publish', help='The folder to publish the (redacted) test results to.')
parser.add_argument('--web', help='The folder to generate the website in.')
parser.add_argument('results', help='The folder to use for the test results.')
args = parser.parse_args()

all_tests = [
    "lxrust-benchs", "lxcpp-benchs", "lxtcutest",
    "rust-net-tests", "cpp-net-tests", "rust-net-benchs", "cpp-net-benchs",
    "rust-algo-tests", "rust-destr-tests", "rust-misc-tests", "rust-vfs-tests",
    "rust-algo-benchs", "rust-misc-benchs", "rust-vfs-benchs",
    "cpp-algo-benchs", "cpp-misc-benchs", "cpp-vfs-benchs",
    "chantests",
    "unittests", "hashmux-benchs", "hashmux-tests", "bench-hashfile-tee", "resmngtest",
    "hello",
    "facever", "rots-raser", "rots-hello", "rots-evidence-test",
    "find", "tar", "untar", "sqlite", "leveldb", "sha256sum", "sort",
    "cat_awk", "cat_wc", "grep_awk", "grep_wc",
    "disk-test", "abort-test",
    "standalone", "libctest", "rust-std-test", "msgchan", "tee-msgchan", "rust-sndrcv", "vmtest",
    "ycsb-bench-udp", "ycsb-bench-tcp",
    "voiceassist-udp", "voiceassist-tcp",
    "bench-shell", "shell-nested", "parchksum", "filterchain",
    # only 1 chain with indirect, because otherwise we would need more than 16 EPs
    "imgproc-indir-1"
]
for num in range(1, 5):
    all_tests.append("imgproc-dir-{}".format(num))

if len(args.tests) == 0:
    args.tests = all_tests

gem5cfg = Path("platform") / "gem5" / "configs" / "m3"
fsimgs = [
    ("default", 32 * 1024, 2048),
    ("bench", 32 * 1024, 4096),
]


class Gem5Test(Test):
    def should_run(self) -> bool:
        # riscv32 does not support VM
        if self.isa == "riscv32" and self.ty != "a":
            return False
        # don't run ROT tests on x86_64, they aren't supported there.
        if self.is_rot_test() and self.isa == "x86_64":
            return False
        # hashmux-{benchs,tests} need the default.py, which uses non-SPM-tiles
        if self.name.startswith("hashmux-") and self.isa == "riscv32":
            return False
        # additionally, rots-raser *only* works on riscv64
        if "raser" in self.name and self.isa != "riscv64":
            return False
        return super().should_run()

    def is_bench(self) -> bool:
        return self.bpe == 64

    def build_env(self, rundir: Path) -> Dict[str, str]:
        vars = super().build_env(rundir)
        vars["M3_TILETYPE"] = "b" if self.ty == "sh" else self.ty
        vars["M3_GEM5_CPU"] = "DerivO3CPU" if self.is_bench() else "TimingSimpleCPU"
        vars["M3_GEM5_LOG"] = "Tcu,TcuRegWrite,TcuCmd,TcuConnector"
        vars["M3_GEM5_CPUFREQ"] = "3GHz"
        vars["M3_GEM5_MEMFREQ"] = "1GHz"

        if self.name != "standalone":
            vars["M3_GEM5_CORES"] = "12"

        if self.is_rot_test():
            fspath = self.build_dir() / "fsimgs-{}".format(self.bpe) / "default.img"
            vars["M3_GEM5_HDD"] = str(fspath)
        else:
            vars["M3_GEM5_CFG"] = str(indir / "test-config.py")

        if self.name.startswith("imgproc"):
            parts = self.name.split('-')
            vars["M3_ACCEL_TYPE"] = "indir" if parts[1] == "indir" else "copy"
            vars["M3_ACCEL_COUNT"] = str(int(parts[2]) * 3)
        elif self.name == "disk-test":
            vars["M3_GEM5_HDD"] = str(indir / "test-hdd.img")
        elif self.name == "abort-test":
            vars["M3_GEM5_CFG"] = str(gem5cfg / "aborttest.py")

        return vars

    def _before_start(self, rundir: Path, boot: Path, vars: Dict[str, str]) -> None:
        # create a run.sh that drops the user into a shell to exactly
        # reproduce and analyze a test
        runfile = str(rundir) + "/run.sh"
        with open(runfile, "w") as f:
            f.write("#!/usr/bin/env bash\n")
            f.write("set -e\n\n")
            # create temp dir
            f.write("tmp=$(mktemp -d)\n")
            f.write("trap 'rm -rf \"$tmp\"' EXIT ERR INT TERM\n")
            f.write("\n")
            # set environment variables
            for var in vars:
                if var != "M3_OUT" and var != "M3_MOD_PATH":
                    f.write("export {}={}\n".format(var, shlex.quote(vars[var])))
            f.write("export M3_MOD_PATH=$tmp\n")
            f.write("\n")
            # rebuild to ensure that mkm3fs is up-to-date
            f.write("./b\n")
            f.write("\n")
            # create FS images
            images = FSImages("gem5")
            for name, blocks, inodes in fsimgs:
                cmd = images.command(self.isa, self.build, self.bpe, name, blocks, inodes,
                                     create_dir=False)
                cmd[1] = "\"$tmp/{}\"".format(os.path.basename(cmd[1]))
                f.write(" ".join(cmd))
                f.write("\n")
            f.write("\n")
            # generate boot script
            f.write("cat > \"$tmp/boot.xml\" <<\"EOF\"\n")
            with open(boot, 'r') as fin:
                for line in fin:
                    f.write(line)
            f.write("EOF\n\n")
            # now we're ready for running/debugging
            f.write("echo \\# You can now run the test via:\n")
            f.write("echo ./b run \"$tmp/boot.xml\"\n")
            f.write("\n")
            f.write("if [ \"$SHELL\" != \"\" ]; then\n")
            f.write("    \"$SHELL\"\n")
            f.write("else\n")
            f.write("    /usr/bin/env bash\n")
            f.write("fi\n")
        os.chmod(runfile, 0o755)

    def _before_exec(self) -> None:
        if self.is_bench():
            vlimit = 12 * 1024 * 1024 * 1024
            tlimit = 40 * 60
        else:
            vlimit = 7 * 1024 * 1024 * 1024
            tlimit = 25 * 60
        resource.setrlimit(resource.RLIMIT_AS, (vlimit, vlimit))
        resource.setrlimit(resource.RLIMIT_CPU, (tlimit, tlimit))


# create FS images
images = FSImages("gem5")
for isa in args.isas:
    for bpe in args.bpes:
        for name, blocks, inodes in fsimgs:
            images.build(isa, "bench", bpe, name, blocks, inodes)

# collect jobs
runner = Runner(Path(args.results))
for test in args.tests:
    for isa in args.isas:
        for bpe in args.bpes:
            for ty in args.types:
                build = "debug" if test == "hello" else "bench"
                t = Gem5Test(test, "gem5", build, isa, ty, bpe)
                if t.should_run():
                    runner.add(t)

# execute everything
try:
    runner.run(os.cpu_count() or 1)
except Exception:
    print(traceback.format_exc())
    print("Stopping tests...")
    runner.stop()
except KeyboardInterrupt:
    print("Stopping tests...")
    runner.stop()

# publish results if we consider the run "successful"
if args.publish:
    pubdir = Path(args.publish)
    pubdir.mkdir(exist_ok=True, parents=True)

    if len(runner.failures) == 0 or (100 * runner.succeeded) / len(runner.failures) >= 90:
        # garbage collect results: remove the results where the commits are no longer reachable
        for filename in os.listdir(pubdir):
            hash = filename[11:]
            gitcmd = ["git", "--no-pager", "branch", "--remotes", "--contains", hash]
            if len(hash) == 40 and subprocess.call(gitcmd,
                                                   stdout=subprocess.DEVNULL,
                                                   stderr=subprocess.DEVNULL) != 0:
                print("Removing '{}' as the commit is no longer reachable.".format(pubdir / filename))
                shutil.rmtree(pubdir / filename)

        # copy all log files to result directory (don't keep gem5 logs etc.)
        subprocess.call("rsync -am --include='log.txt' --include='*/' --exclude='*' {} {}"
                        .format(args.results, pubdir),
                        shell=True)

        # copy coverage results from host tests
        dirname = Path(args.results).name
        subprocess.call(["rsync",
                         "-am",
                         "{}/coverage/".format(args.results),
                         "{}/".format(pubdir / dirname / "coverage")])

        # generate website
        if args.web:
            tests = ",".join(args.tests)
            subprocess.call(["ci/web/generate.py", args.publish, args.web, tests])

# print summary and exit with 0/1
res = runner.summary()
sys.exit(res)
