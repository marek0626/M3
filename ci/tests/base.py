import os
import shutil
import subprocess
import sys
import time

from enum import Enum
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

sys.path.append(os.path.realpath('ci/tests'))  # NOQA
import check_result

indir = Path("ci") / "input"

fstrace_tests = [
    "find", "tar", "untar", "sqlite", "leveldb", "sha256sum", "sort"
]
pipe_tests = [
    "cat_awk", "cat_wc", "grep_awk", "grep_wc"
]
rots_tests = [
    "rots-raser", "rots-hello", "rots-evidence-test",
    "hashmux-benchs", "hashmux-tests", "bench-hashfile-tee",
    "tee-msgchan"
]


class State(Enum):
    INIT = 1
    RUN = 2
    COMPRESS = 3


class Test:
    def __init__(self, name: str, target: str, build: str, isa: str, ty: str, bpe: int) -> None:
        self.name = name
        self.target = target
        self.isa = isa
        self.build = build
        self.ty = ty
        self.bpe = bpe
        self.proc: Optional[subprocess.Popen[Any]] = None
        self.state = State.INIT
        self.retries = 0

    def is_rot_test(self) -> bool:
        return self.name in rots_tests

    def build_dir(self) -> Path:
        return Path("build") / "{}-{}-{}".format(self.target, self.isa, self.build)

    def mod_dir(self) -> Path:
        return self.build_dir() / "fsimgs-{}".format(self.bpe)

    def run_dirname(self) -> str:
        build_tuple = f"{self.target}-{self.isa}-{self.build}"
        return "m3-tests-{}-{}-{}-{}".format(self.name, build_tuple, self.ty, self.bpe)

    def log_file(self, dir: Path) -> Path:
        return dir / self.run_dirname() / "log.txt"

    def gen_boot_script(self, rundir: Path, script: str, env: Dict[str, str]) -> Path:
        shpath = indir / "shared" / script
        defpath = indir / script
        bootfile = rundir / "boot.tmp.xml"
        boot = open(bootfile, "w")
        if self.ty == "sh" and shpath.exists():
            subprocess.run(shpath, stdout=boot, env=env, check=True)
        else:
            subprocess.run(defpath, stdout=boot, env=env, check=True)
        return bootfile

    def boot_script(self, rundir: Path) -> Path:
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

    def step(self, dir: Path) -> bool:
        rundir = dir / self.run_dirname()
        if self.state == State.INIT:
            rundir.mkdir(exist_ok=True, parents=True)
            vars = self.build_env(rundir)
            bootin = self.boot_script(rundir)
            bootgen = rundir / "boot.gen.xml"
            shutil.copyfile(bootin, bootgen)

            self._before_start(rundir, bootin, vars)

            self.proc = subprocess.Popen(["nice", "./b", "-n", "run", bootgen],
                                         stdin=subprocess.DEVNULL,
                                         stdout=subprocess.DEVNULL,
                                         stderr=subprocess.DEVNULL,
                                         env=os.environ.copy() | vars,
                                         preexec_fn=self._before_exec)
            self.state = State.RUN
            return True
        elif self.state == State.RUN:
            assert self.proc
            if self.proc.poll() is None:
                return True
            if self.target == "gem5":
                self.proc = subprocess.Popen(["gzip", "-f", rundir / "gem5.log"])
                self.state = State.COMPRESS
                return True
            else:
                self.proc = None
                self.state = State.INIT
                return False
        elif self.state == State.COMPRESS:
            assert self.proc
            if self.proc.poll() is None:
                return True
            self.proc = None
            self.state = State.INIT
            return False

    def reset(self) -> None:
        self.state = State.INIT
        self.test = None
        self.retries += 1

    def should_run(self) -> bool:
        # standalone works only with SPM
        if self.name == "standalone" and self.ty != "a":
            return False
        # rust-sndrcv and vmtest don't run with SPM
        if (self.name == "rust-sndrcv" or self.name == "vmtest") and self.ty == "a":
            return False
        # m3lx runs only on riscv64 and has no shared version
        if self.name.startswith("lx") and (self.isa != "riscv64" or self.ty != "b"):
            return False
        return True

    def build_env(self, rundir: Path) -> Dict[str, str]:
        vars = {}
        vars["M3_OUT"] = str(rundir)
        vars["M3_TARGET"] = self.target
        vars["M3_ISA"] = self.isa
        vars["M3_BUILD"] = self.build
        vars["M3_MOD_PATH"] = str(self.mod_dir())
        return vars

    def _before_start(self, rundir: Path, boot: Path, vars: Dict[str, str]) -> None:
        pass

    def _before_exec(self) -> None:
        pass


class Runner:
    def __init__(self, dir: Path) -> None:
        self.dir = dir
        self.tests: List[Test] = []
        self.running: List[Test] = []
        self.total = 0
        self.succeeded = 0
        self.finished = 0
        self.failures: List[Tuple[Path, List[check_result.TestResult]]] = []

    def add(self, test: Test) -> None:
        self.tests += [test]

    def run(self, parallel: int) -> None:
        self.total = len(self.tests)
        self.succeeded = 0
        self.finished = 0
        self.failures = []

        # run until there are no more tests to start and all are finished
        while len(self.tests) > 0 or len(self.running) > 0:
            # try to finish the running ones
            i = 0
            while i < len(self.running):
                if not self.running[i].step(self.dir):
                    if self._finish_test(self.running[i], False):
                        self.running.pop(i)
                else:
                    i += 1

            # start new ones until we've reached the limit
            while len(self.tests) > 0 and len(self.running) < parallel:
                t = self.tests.pop(0)
                self._start_test(t)
                self.running.append(t)

            # wait until a child exits
            if len(self.running) > 0:
                time.sleep(0.1)

    def _print_progress(self, line: str) -> None:
        print("[{:3} / {:3}] {}".format(self.finished, self.total, line))

    def _start_test(self, test: Test) -> None:
        test.step(self.dir)
        self._print_progress(f"Started {test.run_dirname()}")

    def _finish_test(self, test: Test, force: bool) -> bool:
        res = check_result.parse_output(test.log_file(self.dir))
        if len(res.failures) == 0:
            self.succeeded += 1
            res_msg = "\033[1;32mSUCCESS\033[0m"
        else:
            self.failures.append((self.dir / test.run_dirname(), res.failures))
            res_msg = "\033[1;31mFAILED\033[0m"
        self.finished += 1
        self._print_progress(f"Finished {test.run_dirname()}: {res_msg}")
        return True

    def stop(self) -> None:
        for run in self.running:
            assert run.proc
            run.proc.terminate()
            self._finish_test(run, True)
        self.running = []

    def summary(self) -> int:
        if self.total - self.succeeded == 0:
            summary = "\033[1mSummary:\033[0m \033[1;32m{} of {} succeeded.\033[0m"
        else:
            summary = "\033[1mSummary:\033[0m \033[1;31m{} of {} succeeded.\033[0m"
        print()
        print(summary.format(self.succeeded, self.total))

        if len(self.failures) > 0:
            print()
            print("The following tests failed:")
            for (path, fails) in self.failures:
                print("{}:".format(path))
                for fail in fails:
                    print("  ", fail)
            return 1
        return 0


class FSImages:
    def __init__(self, target: str) -> None:
        self.target = target

    def build(self, isa: str, build: str, bpe: int, image: str, blocks: int, inodes: int) -> None:
        cmd = self.command(isa, build, bpe, image, blocks, inodes, create_dir=True)
        subprocess.run(cmd, check=True)

    def command(self, isa: str, build: str, bpe: int, image: str,
                blocks: int, inodes: int, create_dir: bool) -> List[str]:
        builddir = Path("build") / f"{self.target}-{isa}-{build}"
        name = f"fsimgs-{bpe}"
        bmoddir = builddir / name
        if create_dir:
            bmoddir.mkdir(exist_ok=True, parents=True)
        return [str(builddir / "toolsbin" / "mkm3fs"),
                str(bmoddir / f"{image}.img"),
                str(builddir / "src" / "fs" / image),
                str(blocks),
                str(inodes),
                str(bpe)]
