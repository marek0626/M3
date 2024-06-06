import argparse
import os
import shutil
import subprocess
import sys

from datetime import datetime
from functools import cmp_to_key

CACHE_CAP = 3


def get_hash(path: str):
    if path == '.':
        res = subprocess.check_output(['git', 'rev-parse', 'HEAD'])
        return res.split()[0].decode()
    res = subprocess.check_output(['git', 'ls-tree', 'HEAD', path])
    return res.split()[2].decode()


def mkdir(path: str):
    os.makedirs(path, exist_ok=True)


class BuildTask:
    def __init__(self, name: str, in_path: str, out_path: str,
                 cmd, shell=False):
        self.name = name
        self.in_path = in_path
        self.out_path = out_path
        self.cmd = cmd
        self.shell = shell

    def hash(self):
        return get_hash(self.in_path)

    def cache_path(self):
        return '{}/{}/{}'.format(args.cache_dir, self.name, self.hash())

    def needs_rebuild(self):
        return not os.path.exists(self.cache_path())

    def get(self, incremental=False):
        rebuild = incremental or self.needs_rebuild()
        if rebuild:
            log, proc = self.start()
            proc.wait()
            log.close()
            if proc.returncode != 0:
                print('{}: exited with status {}'
                      .format(self.name, proc.returncode))
                sys.exit(1)
        self.finish(rebuild, incremental)

    def start(self, incremental=False):
        if not incremental:
            self.gc()

        # create log file
        date = datetime.today().strftime('%Y-%m-%d')
        logfile = '{}/logs/{}/{}-{}.log'.format(
                args.cache_dir, self.name, date, self.hash())
        mkdir(os.path.dirname(logfile))
        log = open(logfile, 'w+')

        # start rebuilding the item
        print("{}: rebuilding... (log in {})".format(self.out_path, logfile))
        sys.stdout.flush()
        proc = subprocess.Popen(self.cmd, shell=self.shell,
                                stdout=log, stderr=log)
        return (log, proc)

    def finish(self, rebuild: bool, incremental: bool):
        print("{}: ready".format(self.out_path))
        sys.stdout.flush()
        if not incremental:
            if rebuild:
                mkdir(os.path.dirname(self.cache_path()))
                subprocess.run(['mv', self.out_path, self.cache_path()])
            if os.path.islink(self.out_path):
                os.unlink(self.out_path)
            elif os.path.isdir(self.out_path):
                shutil.rmtree(self.out_path)
            if os.path.split(self.out_path)[0] != '':
                mkdir(os.path.dirname(self.out_path))
            os.symlink(self.cache_path(), self.out_path)

    def gc(self):
        dir = '{}/{}'.format(args.cache_dir, self.name)
        files = []
        if os.path.isdir(dir):
            for f in os.listdir(dir):
                mtime = os.path.getmtime(os.path.join(dir, f))
                files.append((mtime, f))
            if len(files) > CACHE_CAP:
                sorted_files = sorted(files,
                                      key=cmp_to_key(lambda f1, f2: f2[0] - f1[0]))
                for i in range(CACHE_CAP, len(files)):
                    fpath = os.path.join(dir, sorted_files[i][1])
                    mdate = datetime.utcfromtimestamp(sorted_files[i][0])
                    hdate = mdate.strftime('%Y-%m-%d %H:%M:%S')
                    print('{}: evicting {} (modified on {})...'
                          .format(self.out_path, sorted_files[i][1], hdate))
                    shutil.rmtree(fpath)


def build_all(tasks: [BuildTask], incremental: bool):
    running = []
    for t in tasks:
        if incremental or t.needs_rebuild():
            running.append((t, t.start()))
        else:
            t.finish(False, incremental)
    for (task, (log, proc)) in running:
        proc.wait()
        log.close()
        if proc.returncode != 0:
            print('{}: exited with status {}'
                  .format(task.name, proc.returncode))
            sys.exit(1)
        task.finish(True, incremental)


parser = argparse.ArgumentParser(description='This is the M³ builder.')
parser.add_argument('-i', '--incremental', action='store_true')
parser.add_argument('-c', '--cache-dir', default='ci/out')
parser.add_argument('command')
args = parser.parse_args()

if args.command == 'prepare':
    for m in ['src/m3lx/linux', 'src/m3lx/riscv-pk',
              'tools/ninjapie', 'cross/buildroot',
              'platform/gem5', 'platform/hw',
              'src/libs/leveldb', 'src/libs/musl', 'src/libs/flac',
              'src/apps/bsdutils', 'src/libs/crypto/kecacc-xkcp']:
        t = BuildTask(
            name="submodules/{}".format(os.path.basename(m)),
            in_path=m,
            out_path=m,
            cmd=['git', 'submodule', 'update', '--init', '--recursive', m],
        )
        t.get(args.incremental)

    # disable git hooks for gem5 to avoid user interaction
    subprocess.run(
        'sed --in-place -e "s/return env\\.Entry/return False and env.Entry/" \
                platform/gem5/site_scons/site_tools/git.py',
        shell=True
    )
elif args.command == 'build':
    # build all cross compilers
    tasks = []
    for isa in ['riscv32', 'riscv64', 'x86_64']:
        t = BuildTask(name="build/buildroot-{}".format(isa),
                      in_path='cross/buildroot',
                      out_path='build/cross-{}'.format(isa),
                      cmd='cd cross && ./build.sh {}'.format(isa),
                      shell=True)
        tasks.append(t)

    # build gem5 for all ISAs
    t = BuildTask(name="build/gem5",
                  in_path="platform/gem5",
                  out_path="platform/gem5/build",
                  cmd='cd platform/gem5 && scons -j32 build/{RISCV,X86}/gem5.opt',
                  shell=True)
    tasks.append(t)

    # ensure that we install the requested nightly version for M³
    # and have the target for M³Lx available.
    t = BuildTask(name='build/rustup',
                  in_path='rust-toolchain.toml',
                  out_path='.rustup',
                  cmd=['rustup', 'target', 'add', 'riscv64gc-unknown-linux-gnu'])
    tasks.append(t)

    build_all(tasks, args.incremental)

    # now build M³ for gem5 and all supported ISAs
    tasks = []
    for build in ['debug', 'bench']:
        for isa in ['riscv32', 'riscv64', 'x86_64']:
            t = BuildTask(name='build/m3-gem5-{}-{}'.format(isa, build),
                          in_path='.',
                          out_path='build/gem5-{}-{}'.format(isa, build),
                          cmd='M3_TARGET=gem5 M3_ISA={} M3_BUILD={} ./b'.format(isa, build),
                          shell=True)
            tasks.append(t)

    # build M³Linux for riscv64
    t = BuildTask(name='build/m3lx',
                  in_path='.',
                  out_path='build/linux',
                  cmd='M3_ISA=riscv64 M3_BUILD=bench ./b mklx -n',
                  shell=True)
    tasks.append(t)
    # build bbl separately as it has a different out_path
    t = BuildTask(name='build/riscv-pk',
                  in_path='.',
                  out_path='build/riscv-pk',
                  cmd='M3_ISA=riscv64 M3_BUILD=bench ./b mkbbl -n',
                  shell=True)
    tasks.append(t)

    # actually we cannot use ninja in parallel, because it writes to the same .ninja_log etc..
    # However, this seems to only be a problem when we rebuild later. Thus, we only build in
    # parallel when not doing incremental builds (which is *much* faster).
    if args.incremental:
        for t in tasks:
            t.get(args.incremental)
    else:
        build_all(tasks, args.incremental)
else:
    print("Unknown command '{}'".format(args.command))
