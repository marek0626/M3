#!/usr/bin/env python3

import argparse
import os
import resource
import shutil
import subprocess
import sys

from datetime import datetime
from enum import Enum
from pathlib import Path

sys.path.append(os.path.realpath('ci/tests'))  # NOQA
import check_result

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
    "unittests", "hashmux-benchs", "hashmux-tests", "bench-hashpipe-tee", "resmngtest",
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
fstrace_tests = ["find", "tar", "untar", "sqlite", "leveldb", "sha256sum", "sort"]
pipe_tests = ["cat_awk", "cat_wc", "grep_awk", "grep_wc"]
rots_tests = ["rots-raser", "rots-hello", "rots-evidence-test"]

if len(args.tests) == 0:
    args.tests = all_tests

indir = Path("ci") / "input"
gem5cfg = Path("platform") / "gem5" / "configs" / "m3"


class State(Enum):
    INIT = 1
    RUN = 2
    COMPRESS = 3


class Test:
    def __init__(self, name, target, isa, ty, bpe):
        self.name = name
        self.target = target
        self.isa = isa
        self.ty = ty
        self.bpe = bpe
        self.job = None
        self.state = State.INIT

    def should_run(self):
        # riscv32 does not support VM
        if self.isa == "riscv32" and self.ty != "a":
            return False
        # standalone works only with SPM
        if self.name == "standalone" and self.ty != "a":
            return False
        # don't run ROT tests on x86_64, they aren't supported there.
        if self.name in rots_tests and self.isa == "x86_64":
            return False
        # additionally, rots-raser *only* works on riscv64
        if self.name == "rots-raser" and self.isa == "riscv64":
            return False
        # rust-sndrcv and vmtest don't run with SPM
        if (self.name == "rust-sndrcv" or self.name == "vmtest") and self.ty == "a":
            return False
        # m3lx runs only on riscv64 and has no shared version
        if self.name.startswith("lx") and (self.isa != "riscv64" or self.ty != "b"):
            return False
        return True

    def is_bench(self):
        return self.bpe == 64

    def build_mode(self):
        return "debug" if self.name == "hello" else "bench"

    def build_dir(self):
        return Path("build") / "{}-{}-{}".format(self.target, self.isa, self.build_mode())

    def run_dir(self):
        return "m3-tests-{}-{}-{}-{}".format(self.name, self.ty, self.isa, self.bpe)

    def log_file(self, dir):
        return dir / self.run_dir() / "log.txt"

    def gen_boot_script(self, rundir, script, env):
        shpath = indir / "shared" / script
        defpath = indir / script
        bootfile = rundir / "boot.tmp.xml"
        boot = open(bootfile, "w")
        if self.ty == "sh" and shpath.exists():
            subprocess.run(shpath, stdout=boot, env=env)
        else:
            subprocess.run(defpath, stdout=boot, env=env)
        return bootfile

    def boot_script(self, rundir):
        bootdir = Path("boot")
        if self.name == "abort-test":
            return bootdir / "hello.xml"
        elif self.name.startswith("lx"):
            return bootdir / "linux" / "{}.xml".format(self.name[2:])
        elif self.name in pipe_tests:
            parts = self.name.split('_')
            writer = "{}_{}_{}".format(parts[0], parts[1], parts[0])
            reader = "{}_{}_{}".format(parts[0], parts[1], parts[1])
            vars = os.environ.copy()
            vars["M3_ARGS"] = "-d -i 1 -r 4 -w 1 {} {}".format(writer, reader)
            return self.gen_boot_script(rundir, "bench-scale-pipe.cfg", vars)
        elif self.name.startswith("imgproc"):
            parts = self.name.split('-')
            vars = os.environ.copy()
            vars["M3_ACCEL_TYPE"] = "indir" if parts[1] == "indir" else "copy"
            vars["M3_ACCEL_COUNT"] = str(int(parts[2]) * 3)
            vars["M3_ARGS"] = "-m {} -n {} -w 1 -r 4 /large.txt".format(parts[1], parts[2])
            return self.gen_boot_script(rundir, "imgproc.cfg", vars)
        elif self.name in fstrace_tests:
            vars = os.environ.copy()
            vars["M3_ARGS"] = "-n 4 -t -d -u 1 {}".format(self.name)
            return self.gen_boot_script(rundir, "fstrace.cfg", vars)
        else:
            name = "{}.xml".format(self.name)
            shpath = bootdir / "shared" / name
            defpath = bootdir / name
            if self.ty == "sh" and shpath.exists():
                return shpath
            else:
                return defpath

    def build_env(self, rundir):
        vars = {}
        vars["M3_OUT"] = str(rundir)
        vars["M3_TARGET"] = self.target
        vars["M3_ISA"] = self.isa
        vars["M3_BUILD"] = self.build_mode()
        vars["M3_MOD_PATH"] = str(self.build_dir() / "fsimgs-{}".format(self.bpe))
        vars["M3_TILETYPE"] = "b" if self.ty == "sh" else self.ty
        vars["M3_GEM5_CPU"] = "DerivO3CPU" if self.is_bench() else "TimingSimpleCPU"
        vars["M3_GEM5_LOG"] = "Tcu,TcuRegWrite,TcuCmd,TcuConnector"
        vars["M3_GEM5_CPUFREQ"] = "3GHz"
        vars["M3_GEM5_MEMFREQ"] = "1GHz"

        if self.name != "standalone":
            vars["M3_GEM5_CORES"] = "12"

        if self.name in rots_tests:
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

    def __call__(self):
        if self.is_bench():
            vlimit = 12 * 1024 * 1024 * 1024
            tlimit = 40 * 60
        else:
            vlimit = 7 * 1024 * 1024 * 1024
            tlimit = 25 * 60
        resource.setrlimit(resource.RLIMIT_AS, (vlimit, vlimit))
        resource.setrlimit(resource.RLIMIT_CPU, (tlimit, tlimit))

    def step(self, dir):
        rundir = Path(dir) / self.run_dir()
        if self.state == State.INIT:
            rundir.mkdir(exist_ok=True, parents=True)
            vars = self.build_env(rundir)
            bootin = self.boot_script(rundir)
            bootgen = rundir / "boot.gen.xml"
            shutil.copyfile(bootin, bootgen)
            self.job = subprocess.Popen(["nice", "./b", "run", bootgen, "-n"],
                                        stdin=subprocess.DEVNULL,
                                        stdout=subprocess.DEVNULL,
                                        stderr=subprocess.DEVNULL,
                                        env=os.environ.copy() | vars,
                                        preexec_fn=self)
            self.state = State.RUN
            return True
        elif self.state == State.RUN:
            if self.job.poll() is None:
                return True
            self.job = subprocess.Popen(["gzip", "-f", rundir / "gem5.log"])
            self.state = State.COMPRESS
            return True
        elif self.state == State.COMPRESS:
            if self.job.poll() is None:
                return True
            self.job = None
            self.state = State.INIT
            return False


class Jobs:
    def __init__(self, dir):
        self.dir = Path(dir)
        self.jobs = []
        self.running = []
        self.total = 0
        self.succeeded = 0
        self.finished = 0
        self.failures = []

    def add(self, job):
        self.jobs += [job]

    def run(self, parallel):
        self.total = len(self.jobs)
        self.succeeded = 0
        self.finished = 0
        self.failures = []

        # run until there are no more jobs to start and all are finished
        while len(self.jobs) > 0 or len(self.running) > 0:
            # try to finish the running ones
            i = 0
            while i < len(self.running):
                if not self.running[i].step(self.dir):
                    self._finish_job(self.running[i])
                    self.running.pop(i)
                else:
                    i += 1

            # start new ones until we've reached the limit
            while len(self.jobs) > 0 and len(self.running) < parallel:
                t = self.jobs.pop(0)
                self._start_job(t)
                self.running.append(t)

            # wait until a child exits
            if len(self.running) > 0:
                os.waitpid(-1, 0)

    def _start_job(self, job):
        job.step(self.dir)
        print("[{:3} / {:3}] Started {}".format(self.finished, self.total, job.run_dir()))

    def _finish_job(self, job):
        res = check_result.parse_output(job.log_file(self.dir))
        if len(res.failures) == 0:
            self.succeeded += 1
            msg = "[{:3} / {:3}] Finished {}: \033[1;32mSUCCESS\033[0m"
        else:
            self.failures.append((self.dir / job.run_dir(), res.failures))
            msg = "[{:3} / {:3}] Finished {}: \033[1;31mFAILED\033[0m"
        print(msg.format(self.finished, self.total, job.run_dir()))
        self.finished += 1

    def stop(self):
        for run in self.running:
            run.job.kill()
            self._finish_job(run)
        self.running = []


# create FS images
for isa in args.isas:
    builddir = Path("build") / "gem5-{}-{}".format(isa, "bench")
    for bpe in args.bpes:
        bmoddir = builddir / "fsimgs-{}".format(bpe)
        bmoddir.mkdir(exist_ok=True, parents=True)
        subprocess.run([builddir / "toolsbin" / "mkm3fs",
                        bmoddir / "bench.img",
                        builddir / "src" / "fs" / "bench",
                        str(64 * 1024),  # blocks
                        str(4096),       # inodes
                        str(bpe)])
        subprocess.run([builddir / "toolsbin" / "mkm3fs",
                        bmoddir / "default.img",
                        builddir / "src" / "fs" / "default",
                        str(16 * 1024),  # blocks
                        str(512),        # inodes
                        str(bpe)])

# collect jobs
jobs = Jobs(args.results)
for test in args.tests:
    for isa in args.isas:
        for bpe in args.bpes:
            for ty in args.types:
                t = Test(test, "gem5", isa, ty, bpe)
                if t.should_run():
                    jobs.add(t)

# execute everything
try:
    jobs.run(os.cpu_count())
except (KeyboardInterrupt, Exception):
    print("Stopping tests...")
    jobs.stop()

# publish results if we consider the run "successful"
if args.publish:
    pubdir = Path(args.publish)
    pubdir.mkdir(exist_ok=True, parents=True)

    if len(jobs.failures) == 0 or (100 * jobs.succeeded) / len(jobs.failures) >= 90:
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
            subprocess.call(["ci/web/generate.py", args.publish, args.web])

# print summary
if jobs.total - jobs.succeeded == 0:
    summary = "\033[1mSummary:\033[0m \033[1;32m{} of {} succeeded.\033[0m"
else:
    summary = "\033[1mSummary:\033[0m \033[1;31m{} of {} succeeded.\033[0m"
print()
print(summary.format(jobs.succeeded, jobs.total))

# print failures
if len(jobs.failures) > 0:
    print()
    print("The following tests failed:")
    for (name, fails) in jobs.failures:
        print("{}:".format(name))
        for fail in fails:
            print("  ", fail)
    sys.exit(1)
