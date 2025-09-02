#!/usr/bin/env python3

from git import Repo
import subprocess
import sys

RETRIES = 10

branches = {
    'ninjapie': 'master',
    'platform/hw': 'master',
    'tools/lints': 'main',
}


def desired_commit(path: str) -> str:
    commit = subprocess.check_output(["git", "ls-tree", "HEAD", path]).decode().split()[2]
    return commit


def actual_commit(url: str, branch: str) -> str:
    last_err = None
    for i in range(0, RETRIES):
        try:
            commit = subprocess.check_output(["git", "ls-remote", mod.url, branch],
                                             stderr=subprocess.DEVNULL).decode().split()[0]
            return commit
        except Exception as e:
            last_err = e
            pass
    raise Exception("Submodule {}: {}".format(mod.name, last_err))


res = 0
repo = Repo('.')
for mod in repo.submodules:
    branch = 'm3' if mod.name not in branches else branches[mod.name]
    desired = desired_commit(str(mod.path))
    actual = actual_commit(mod.url, branch)
    if actual != desired:
        print("{}: expected commit {}, found {} in branch {}"
              .format(mod.path, desired, actual, branch))
        res = 1
sys.exit(res)
