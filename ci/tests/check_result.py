#!/usr/bin/env python3

import math
import re
import sys

from pathlib import Path
from typing import AnyStr, Dict, List


def convert_unit(number: float, dst_unit: str, src_unit: str) -> float:
    unit_conv = {
        'ns': 1_000_000_000.0,
        'us': 1_000_000.0,
        'ms': 1_000.0,
        's': 1.0,
        'cycles': 1.0,
    }
    return number * (unit_conv[dst_unit] / unit_conv[src_unit])


class PerfResult:
    def __init__(self, name: str, time: float, unit: str, variance: float, runs: int) -> None:
        self.name = name
        self.time = time if not math.isinf(time) and not math.isnan(time) else None
        self.unit = unit
        self.variance = variance
        self.runs = runs

    def __repr__(self) -> str:
        res = f"PERF[{self.name}] = {self.time} {self.unit}"
        res += " ({self.variance} with {self.runs} runs)\n"
        return res


class TestResult:
    def __init__(self, name: str, desc: str) -> None:
        self.name = name
        self.desc = desc

    def __repr__(self) -> str:
        if self.name == "":
            return self.desc
        return f"{self.name}: {self.desc}"


class Result:
    def __init__(self) -> None:
        self.failed_tests = 0
        self.succ_tests = 0
        self.failures: List[TestResult] = []
        self.perfs: Dict[str, PerfResult] = {}

    def add_failed_test(self, name: str, desc: str) -> None:
        self.failures.append(TestResult(name, desc))

    def add_perf(self, pmatch: re.Match[AnyStr]) -> None:
        name = re.sub(r"^.*/([^/]+)$", r"\1", str(pmatch.group(1))) + ": " + str(pmatch.group(3))
        res_unit = pmatch.group(5)
        var_unit = pmatch.group(7)
        if res_unit is not None and var_unit is not None:
            variance = convert_unit(float(pmatch.group(6)),
                                    str(res_unit.strip()),
                                    str(var_unit.strip()))
        else:
            variance = float(pmatch.group(6))
        self.perfs[name] = PerfResult(name,
                                      float(pmatch.group(4)),
                                      str(pmatch.group(5)),
                                      variance,
                                      int(pmatch.group(8)))

    def __repr__(self) -> str:
        str = f"{self.failed_tests} / {self.succ_tests + self.failed_tests} succeeded"
        if len(self.perfs) > 0:
            str += "\n"
            for p in self.perfs:
                str += "  " + repr(self.perfs[p])
        return str


re_test = re.compile(r'^Testing "(.*?)" in (.*?):$')
re_failed = re.compile(r'^!\s+([^:]+):(\d+)\s+(.*?) FAILED$')
re_perf = re.compile(
    r'^.*!\s+([^:]+):(\d+)\s+PERF\s+"(.*?)": ([\d\.]+) (\S+?) \(\+/\- ([0-9\-\.]+)( \S+)? with (\d+) runs\)$')
re_shdn = re.compile(r'^.*\[(PE0:\S+\s*@\s*\d+|\S+\s*@.*?)\].*Shutting down$')
re_fsck = re.compile(r'^.*(m3fsck:.*)$')
re_exit = re.compile(r'^.*Child .*? exited with exitcode')
re_panic = re.compile(r'^.*PANIC at(.*)$')


def parse_output(file: Path) -> Result:
    failed_asserts = 0
    res = Result()
    seen_shutdown = False
    seen_fsck = ''
    with open(file, 'r', errors='replace') as reader:
        line = reader.readline()
        test = ""
        while line != '':
            line = line.strip()
            # remove escape codes from line; otherwise the regular expressions don't work
            line = re.sub(r"\033\[.*?m", '', line)
            # special handling for the TCU abort test
            if line.startswith("info: "):
                line = line[6:]
            tmatch = re_test.match(line)
            if tmatch:
                if test != "":
                    if failed_asserts == 0:
                        res.succ_tests += 1
                    else:
                        res.failed_tests += 1
                    failed_asserts = 0
                test = tmatch.group(1)
            else:
                fmatch = re_failed.match(line)
                if fmatch:
                    res.add_failed_test(fmatch.group(1) + ":" + fmatch.group(2), fmatch.group(3))
                    failed_asserts += 1
                else:
                    pmatch = re_perf.match(line)
                    if pmatch:
                        res.add_perf(pmatch)
                        res.succ_tests += 1
                    elif re_shdn.match(line):
                        seen_shutdown = True
                    elif re_exit.match(line):
                        res.failed_tests += 1
                        res.add_failed_test("", line)
                    elif re_fsck.match(line):
                        fsck_match = re_fsck.match(line)
                        if fsck_match:
                            seen_fsck = fsck_match.group(1)
                    else:
                        panic_match = re_panic.match(line)
                        if panic_match:
                            res.add_failed_test("", "PANIC at " + panic_match.group(1))
                            res.failed_tests += 1

            line = reader.readline()
    if not seen_shutdown:
        res.failed_tests += 1
        res.add_failed_test("", "Test did not complete (no kernel shutdown)")
    if seen_fsck != '':
        res.failed_tests += 1
        res.add_failed_test("", seen_fsck)
    return res


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print("Usage: {} <file>".format(sys.argv[0]))
        sys.exit(1)

    res = parse_output(Path(sys.argv[1]))
    for failed in res.failures:
        print("  {} \033[1mfailed\033[0m".format(failed), file=sys.stderr)

    sys.exit(0 if res.failed_tests == 0 else 1)
