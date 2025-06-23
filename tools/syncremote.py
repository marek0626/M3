#!/usr/bin/env python3

import argparse
import asyncio
import os
import shlex
import subprocess
import tempfile
import traceback

excludes = [
    '.git',
    '.gitmodules',
    '__pycache__',
    '/backup',
    '/build',
    '/cross/buildroot/dl',
    '/run',
    '/.envrc',
    '/.ninja_deps',
    '/.ninja_log',
    '/.nvim.lua',
    '/nix/.direnv',
    '/.cache',
    '/.cargo',
    '/.rustup',
    '/.dylint_drivers',
]


class FileCollector:
    def __init__(self, timeout_seconds, on_timeout):
        self.timeout_seconds = timeout_seconds
        self.on_timeout = on_timeout
        self.created = set()
        self.modified = set()
        self.deleted = set()
        self.moved = set()
        self._timeout_task = None
        self._lock = asyncio.Lock()

    async def handle_timeout(self):
        await asyncio.sleep(self.timeout_seconds)
        async with self._lock:
            if self.created or self.modified or self.deleted or self.moved:
                try:
                    # sync union of created and modified files
                    files = [f for f in self.created.union(self.modified)]
                    dirs = set([os.path.dirname(f) for f in self.deleted])
                    dirs = list(dirs.union(self.moved))
                    self.on_timeout(dirs, files)
                except Exception:
                    print(traceback.format_exc())
                self.created.clear()
                self.modified.clear()
                self.deleted.clear()
                self.moved.clear()
            self._timeout_task = None

    async def process_line(self, line):
        events = [
            "CREATE", "MODIFY", "DELETE",
            "MOVED_FROM", "MOVED_TO",
            "MOVED_FROM,ISDIR", "MOVED_TO,ISDIR"
        ]
        async with self._lock:
            parts = line.split(' ')
            # print(line)
            if parts[0] not in events:
                return
            full_path = parts[1] + parts[2]
            # skip temporary files
            if full_path.endswith('~'):
                return
            match parts[0]:
                case "CREATE":
                    # if we recreated the file, don't sync the directory
                    if full_path in self.deleted:
                        self.deleted.remove(full_path)
                        self.modified.add(full_path)
                    else:
                        self.created.add(full_path)
                case "MODIFY":
                    self.modified.add(full_path)
                case "DELETE":
                    # if it was created in this run, we can ignore it completely
                    if full_path in self.created:
                        if full_path in self.modified:
                            self.modified.remove(full_path)
                        self.created.remove(full_path)
                    else:
                        self.deleted.add(full_path)
                case "MOVED_FROM" | "MOVED_TO" | "MOVED_FROM,ISDIR" | "MOVED_TO,ISDIR":
                    # if something was moved, we just update the directory
                    self.moved.add(parts[1])
            # print(self.created, self.modified, self.deleted, self.moved)
            if self._timeout_task is None:
                self._timeout_task = asyncio.create_task(self.handle_timeout())


async def sync_incremental(dest, timeout_seconds, dry_run):
    def perform_sync(dirs, files):
        print('Syncing: dirs={}, files={} ... '.format(dirs, files),
              flush=True, end='')
        with tempfile.NamedTemporaryFile(delete_on_close=False) as fp:
            # write dirs and files to sync in temp file
            for f in dirs:
                fp.write("{}/\n".format(f).encode('utf-8'))
            for f in files:
                fp.write("{}\n".format(f).encode('utf-8'))
            fp.close()

            cmd = ['rsync', '--compress', '--links', '--perms', '--delete']
            if dry_run:
                cmd += ['--dry-run', '--itemize-changes']
            cmd += ['--files-from={}'.format(shlex.quote(fp.name))]
            cmd += ['.', dest]
            subprocess.run(cmd, stdout=subprocess.DEVNULL)
        print('DONE')

    def posix_regex(s):
        res = s
        if res.startswith('/'):
            res = '^./' + res[1:]
        else:
            res = './' + res
        return res.replace('.', r'\.').replace('/', r'\/')

    collector = FileCollector(timeout_seconds, perform_sync)

    exclude_list = [posix_regex(e) for e in excludes]
    cmd = [
        'inotifywait',
        '--recursive', '--monitor', '--format', '%e %w %f',
        '.',
        # add '/' suffix to ensure we really exclude exactly this file/directory and not every
        # file/directory that starts with it
        '--exclude=(' + '|'.join([e + '/' for e in exclude_list]) + ')',
        '--event', 'modify', '--event', 'create', '--event', 'delete', '--event', 'move',
    ]
    proc = await asyncio.create_subprocess_exec(*cmd, stdout=asyncio.subprocess.PIPE)
    while True:
        line = await proc.stdout.readline()
        if not line:
            break
        line = line.decode().rstrip()
        await collector.process_line(line)


def remote_cmd(target, local_cmd):
    host, path = tuple(target.split(':'))
    local_cmd = [shlex.quote(c) for c in local_cmd]
    return ['ssh', host, 'cd {} && '.format(shlex.quote(path)) + ' '.join(local_cmd)]


def ask_user(question):
    answer = input(question + "Are you sure to continue (y/n)? ")
    return answer == 'y'


def check_commit(src, dest):
    target = src if dest == './' else dest
    local_commit_cmd = ['git', 'rev-parse', 'HEAD']
    remote_commit_cmd = remote_cmd(target, local_commit_cmd)

    # get local commit
    try:
        local_commit = subprocess.check_output(local_commit_cmd).decode().strip()
    except Exception:
        print(traceback.format_exc())
        return False

    # get remote commit
    remote_commit = None
    try:
        remote_commit = subprocess.check_output(remote_commit_cmd).decode().strip()
    except Exception:
        # syncing to a remote build machine is fine without a git repository on the remote side
        if src == './':
            if not ask_user("Unable to determine commit at remote side. "):
                return False
        # as the user is not expected to work on the build machine, syncing to local implies that
        # the remote machine is also used for working and therefore should be a valid git repo
        else:
            print(traceback.format_exc())
            return False

    # are we at the same commit?
    if remote_commit is not None and remote_commit != local_commit:
        if not ask_user("The local repository is at a different commit ({}) than the"
                        " remote repository ({}). ".format(local_commit, remote_commit)):
            return False
    return True


def check_changes(src, dest):
    local_changes_cmd = ['git', 'status', '--porcelain']

    # sync to local
    if dest == './':
        changes = subprocess.check_output(local_changes_cmd).decode().strip()
        if changes != "":
            if not ask_user("The local working directory is not clean:\n{}\n".format(changes)):
                return False
    # sync to remote
    else:
        remote_changes_cmd = remote_cmd(dest, local_changes_cmd)
        try:
            changes = subprocess.check_output(remote_changes_cmd).decode().strip()
            if changes != "":
                if not ask_user("The remote working directory is not clean:\n{}\n".format(changes)):
                    return False
        except Exception:
            # this might be fine if the remote side is no git repository
            if not ask_user("Unable to determine repository state at remote side. "):
                return False
    return True


def sync_full(src, dest, dry_run, force):
    # some safety checks to ensure local and remote are in a sane state for this sync
    if (not dry_run or force) and (not check_commit(src, dest) or not check_changes(src, dest)):
        return

    cmd = ['rsync']
    cmd += ['--compress', '--links', '--perms', '--times', '--delete', '--recursive']
    if dry_run:
        cmd += ['--dry-run', '--itemize-changes']
    else:
        cmd += ['--verbose']
    cmd += ['--exclude={}'.format(e) for e in excludes]
    cmd += [src, dest]
    subprocess.run(cmd)


parser = argparse.ArgumentParser(description='This is the M³ remote syncer.')
parser.add_argument('remote', help='The remote location to sync to (e.g., bios:M3)')
parser.add_argument('operation', choices=['incremental', 'to_remote', 'to_local'],
                    default='incremental',
                    help='The operation to execute; "incremental" monitors the repository and syncs'
                    ' all changes to the remote site, whereas "to_remote" and "to_local" perform a'
                    ' single synchronization from local to remote or vice versa.')
parser.add_argument('--dry-run', action='store_true',
                    help='Perform a dry run and only show the changes')
parser.add_argument('--force', action='store_true',
                    help='Do not ask on changes or different commits, just sync')
args = parser.parse_args()

if not args.remote.endswith('/'):
    args.remote += '/'

try:
    match args.operation:
        case 'incremental':
            asyncio.run(sync_incremental(args.remote, 10e-3, args.dry_run))
        case 'to_remote':
            sync_full('./', args.remote, args.dry_run, args.force)
        case 'to_local':
            sync_full(args.remote, './', args.dry_run, args.force)
except KeyboardInterrupt:
    pass
except Exception:
    print(traceback.format_exc())
