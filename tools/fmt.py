#!/usr/bin/env python3

# M³'s asynchronous formatter
#
# Copyright (C) 2025 Viktor Reusch, Barkhausen Institut

import os
import asyncio
from asyncio.subprocess import PIPE, STDOUT
import argparse


async def main():
    parser = argparse.ArgumentParser(description="M³'s asynchronous formatter")
    parser.add_argument(
        "-i",
        "--inplace",
        action="store_true",
        help="replace files with formatted version",
    )
    args = parser.parse_args()

    inplace = args.inplace
    routines = []

    def my_fmt(path):
        nonlocal routines, inplace
        routines += fmt(path, inplace)

    walk(my_fmt)

    results = await asyncio.gather(*routines)
    if not all(results):
        exit(1)


def walk(func):
    for root, dirs, files in os.walk("."):
        for d in list(dirs):
            keep = all((filt(root, d) for filt in FILTERS))
            if not keep:
                dirs.remove(d)

        for f in list(files):
            if not all((filt(root, f) for filt in FILTERS)):
                continue
            func(os.path.join(root, f))
    func("./b")


def root_filter(root, name):
    DIRS = ["ci", "src", "tools", "boot", "cross", ".gitlab"]
    return root != "." or name in DIRS


def dir_filter(root, name):
    DIRS = ["src/m3lx", "cross/buildroot", "tools/ninjapie", "tools/lints"]
    path = os.path.join(root, name)
    for d in DIRS:
        if path.endswith("/" + d):
            return False
    return True


def build_filter(root, name):
    DIRS = ["src/libs/flac",  "src/apps/bsdutils", "src/libs/leveldb", "src/libs/axieth"]
    for d in DIRS:
        if root.endswith("/" + d) and name != "build.py":
            return False
    return True


def musl_filter(root, name):
    return not root.endswith("/src/libs/musl") or name == "build.py" or name == "m3"


FILTERS = [root_filter, dir_filter, build_filter, musl_filter]


def fmt(*args, **kwargs):
    return [f(*args, **kwargs) for f in FORMATTERS]


async def clang_fmt(path, inplace):
    if not path.endswith(".cc") and not path.endswith(".h"):
        return True
    args = ["-i"] if inplace else ["--dry-run", "--Werror"]
    return await exec(path, ["clang-format"] + args + [path])


async def cargo_fmt(path, inplace):
    if not path.endswith("/Cargo.toml"):
        return True
    args = [] if inplace else ["--check"]

    routines = []
    dirname = os.path.dirname(path)
    for root, _, files in os.walk(os.path.join(dirname, "src")):
        for f in files:
            if not f.endswith(".rs"):
                continue
            path = os.path.join(root, f)
            routines.append(exec(path, ["rustfmt"] + args + [path]))

    results = await asyncio.gather(*routines)
    return all(results)


async def python_fmt(path, inplace):
    if not path.endswith(".py"):
        return True
    args = ["-i"] if inplace else ["--diff", "--exit-code"]
    return await exec(path, ["autopep8", "--global-config", ".python-format"] + args + [path])


async def xml_fmt(path, inplace):
    if not path.endswith(".xml"):
        return True
    args = ["--inplace"] if inplace else []
    env = os.environ.copy()
    env["XMLLINT_INDENT"] = "    "
    return await exec(path, ["./tools/wrapfmt.py"] + args + ["xmllint --format", path], env=env)


async def shell_fmt(path, inplace):
    if not path.endswith(".sh") and path != "./b":
        return True
    args = ["--indent", "4", "--case-indent"]
    args += ["--write"] if inplace else ["--diff"]
    return await exec(path, ["shfmt"] + args + [path])


async def yaml_fmt(path, inplace):
    if not path.endswith(".yaml") and not path.endswith(".yml"):
        return True
    args = ["-conf", ".yamlfmt.yaml"]
    args += [] if inplace else ["--lint"]
    return await exec(path, ["yamlfmt"] + args + [path])

limiter = asyncio.Semaphore(128)


async def exec(path, args, **kwargs):
    async with limiter:
        proc = await asyncio.create_subprocess_exec(*args, **kwargs, stdout=PIPE, stderr=STDOUT)
        stdout, _ = await proc.communicate()
        print(f"Formatting {path}...")
        print(stdout.decode(), end="")
        return proc.returncode == 0


FORMATTERS = [clang_fmt, cargo_fmt, python_fmt, xml_fmt, shell_fmt, yaml_fmt]


if __name__ == "__main__":
    asyncio.run(main())
