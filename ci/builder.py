import argparse
import os
import re
import shutil
import subprocess
import sys

from datetime import datetime
from functools import cmp_to_key

CACHE_CAP = 3


def get_hash(path: str):
    # for the root of the repository (current directory), we don't find the hash via ls-tree, but
    # have to use rev-parse instead.
    if path == '.':
        res = subprocess.check_output(['git', 'rev-parse', 'HEAD'])
        return res.split()[0].decode()

    # here we receive: <perm> <type> <hash> <name>
    res = subprocess.check_output(['git', 'ls-tree', 'HEAD', path])
    return res.split()[2].decode()


def mkdir(path: str):
    os.makedirs(path, exist_ok=True)


def gc_dir(dir: str, max: int):
    files = []
    if os.path.isdir(dir):
        # collect folder items including the last modification time. note that the last access
        # time would be better, but we would need to track that manually and that's maybe not
        # worth the trouble.
        for f in os.listdir(dir):
            mtime = os.path.getmtime(os.path.join(dir, f))
            files.append((mtime, f))

        # if we're at the limit, evict the least recently modified entries
        if len(files) > max:
            sorted_files = sorted(files,
                                  key=cmp_to_key(lambda f1, f2: f2[0] - f1[0]))
            for i in range(max, len(files)):
                fpath = os.path.join(dir, sorted_files[i][1])
                mdate = datetime.utcfromtimestamp(sorted_files[i][0])
                hdate = mdate.strftime('%Y-%m-%d %H:%M:%S')
                print('{}: evicting (modified on {})...'.format(fpath, hdate))
                if os.path.isfile(fpath):
                    os.unlink(fpath)
                elif os.path.isdir(fpath):
                    shutil.rmtree(fpath)


class BuildTask:
    def __init__(self, name: str, in_path: str, out_path: str, cache_dir: str,
                 cmd, shell=False, werror=False):
        self.name = name
        self.in_path = in_path
        self.out_path = out_path
        self.cache_dir = cache_dir
        self.cmd = cmd
        self.shell = shell
        self.werror = werror

    def hash(self):
        return get_hash(self.in_path)

    def cache_path(self):
        return '{}/{}/{}'.format(self.cache_dir, self.name, self.hash())

    def needs_rebuild(self):
        return not os.path.exists(self.cache_path())

    def get(self, incremental=False):
        log = None
        returncode = 0

        # in incremental mode, we always want to build, because most of the time it is not actually
        # a complete rebuild.
        rebuild = incremental or self.needs_rebuild()
        if rebuild:
            # start and synchronously wait for the process to finish
            log, proc = self.start(incremental)
            proc.wait()
            returncode = proc.returncode

        self.finish(rebuild, incremental, returncode, log)

    def start(self, incremental=False):
        # evict entries from the cache (in incremental mode we are not using the cache), if
        # required.
        if not incremental:
            self.gc()

        # create log file
        if incremental:
            logfile = '{}/logs/{}.log'.format(self.cache_dir, self.name)
        else:
            date = datetime.today().strftime('%Y-%m-%d')
            logfile = '{}/logs/{}/{}-{}.log'.format(
                    self.cache_dir, self.name, date, self.hash())
        mkdir(os.path.dirname(logfile))
        log = open(logfile, 'w+')

        # start rebuilding the item
        print("{}: rebuilding... (log in {})".format(self.out_path, logfile))
        sys.stdout.flush()
        proc = subprocess.Popen(self.cmd, shell=self.shell,
                                stdout=log, stderr=log)
        return (log, proc)

    def detect_errors(self, log):
        log.seek(0)
        errors = False
        seen_ninja = False
        for line in log.readlines():
            # all lines that don't start with the ninja build progress are considered
            # warnings/errors. thus, print them here and exit with failure afterwards
            if re.search(r'^\[\d+/\d+\]', line):
                seen_ninja = True
            elif seen_ninja:
                if not errors:
                    print("{}: build failed:".format(self.out_path))
                sys.stdout.write(line)
                errors = True
        if errors:
            sys.exit(1)

    def finish(self, rebuild: bool, incremental: bool, returncode: int, log):
        # check for errors
        if rebuild:
            if self.werror:
                self.detect_errors(log)
            if returncode != 0:
                print('{}: exited with status {}'
                      .format(self.out_path, returncode))
                sys.exit(1)
            log.close()

        print("{}: ready".format(self.out_path))
        sys.stdout.flush()

        if not incremental:
            # if we rebuilt, we move the out directory into the cache
            if rebuild:
                mkdir(os.path.dirname(self.cache_path()))
                subprocess.run(['mv', self.out_path, self.cache_path()])
            # make sure that the out directory does not exist
            if os.path.islink(self.out_path):
                os.unlink(self.out_path)
            elif os.path.isdir(self.out_path):
                shutil.rmtree(self.out_path)
            # ensure that at least the parent directory exist
            if os.path.split(self.out_path)[0] != '':
                mkdir(os.path.dirname(self.out_path))
            # now link to the cache
            os.symlink(self.cache_path(), self.out_path)

    def gc(self):
        dir = '{}/{}'.format(self.cache_dir, self.name)
        gc_dir(dir, CACHE_CAP - 1)


def build_all(tasks: [BuildTask], incremental: bool):
    # start all tasks that have to run and let them run in parallel
    running = []
    for t in tasks:
        # start task (incremental always (re)builds)
        if incremental or t.needs_rebuild():
            running.append((t, t.start(incremental)))
        # otherwise, finish the task right away
        else:
            t.finish(False, incremental, 0, None)

    # now wait for all tasks
    for (task, (log, proc)) in running:
        proc.wait()
        task.finish(True, incremental, proc.returncode, log)


def prepare(targets: [str], isas: [str], cache_dir: str, incremental: bool):
    # determine required submodules
    mods = ['tools/ninjapie', 'tools/lints', 'cross/buildroot',
            'src/libs/leveldb', 'src/libs/musl', 'src/libs/flac',
            'src/apps/bsdutils', 'src/libs/crypto/kecacc-xkcp']
    if 'riscv64' in isas:
        mods.append('src/m3lx/linux')
        mods.append('src/m3lx/riscv-pk')
    if 'gem5' in targets:
        mods.append('platform/gem5')
    if 'hw22' in targets or 'hw23' in targets:
        mods.append('platform/hw')

    # pull in required submodules
    for m in mods:
        t = BuildTask(
            name="submodules/{}".format(os.path.basename(m)),
            in_path=m,
            out_path=m,
            cache_dir=cache_dir,
            cmd=['git', 'submodule', 'update', '--init', '--recursive', m],
        )
        t.get(incremental)


def build(targets: [str], isas: [str], builds: [str], cache_dir: str,
          incremental: bool):
    # when we build for riscv64, we always need the riscv32 toolchain as well to run stuff on the
    # accelerator co-processors
    ccisas = isas.copy()
    if 'riscv64' in ccisas and 'riscv32' not in ccisas:
        ccisas.append('riscv32')

    # build all cross compilers
    tasks = []
    for isa in ccisas:
        t = BuildTask(name="build/buildroot-{}".format(isa),
                      in_path='cross/buildroot',
                      out_path='build/cross-{}'.format(isa),
                      cache_dir=cache_dir,
                      cmd='cd cross && ./build.sh {}'.format(isa),
                      shell=True)
        tasks.append(t)

    if 'gem5' in targets:
        # build gem5 for all ISAs
        gem5isas = []
        if 'riscv32' in isas or 'riscv64' in isas:
            gem5isas.append('RISCV')
        if 'x86_64' in isas:
            gem5isas.append('X86')
        gem5isas = gem5isas[0] if len(gem5isas) == 1 else '{' + ','.join(gem5isas) + '}'
        t = BuildTask(name="build/gem5",
                      in_path="platform/gem5",
                      out_path="platform/gem5/build",
                      cache_dir=cache_dir,
                      cmd='cd platform/gem5 && scons -j32 build/' + gem5isas + '/gem5.opt',
                      shell=True)
        tasks.append(t)

    # ensure that we install the requested nightly version for M³
    # and have the target for M³Lx available.
    t = BuildTask(name='build/rustup',
                  in_path='rust-toolchain.toml',
                  out_path='.rustup',
                  cache_dir=cache_dir,
                  cmd=['rustup', 'target', 'add', 'riscv64gc-unknown-linux-gnu'])
    tasks.append(t)

    build_all(tasks, incremental)

    # now build M³ for all targets, build types, and ISAs
    tasks = []
    for tgt in targets:
        for build in builds:
            for isa in isas:
                if tgt != 'gem5' and isa != 'riscv64':
                    continue
                t = BuildTask(name='build/m3-{}-{}-{}'.format(tgt, isa, build),
                              in_path='.',
                              out_path='build/{}-{}-{}'.format(tgt, isa, build),
                              cache_dir=cache_dir,
                              cmd='M3_TARGET={} M3_ISA={} M3_BUILD={} ./b'.format(tgt, isa, build),
                              shell=True,
                              werror=True)
                tasks.append(t)

    # build M³Linux for riscv64
    if 'riscv64' in isas:
        t = BuildTask(name='build/m3lx',
                      in_path='.',
                      out_path='build/linux',
                      cache_dir=cache_dir,
                      cmd='M3_ISA=riscv64 M3_BUILD=bench ./b mklx -n',
                      shell=True)
        tasks.append(t)

    # actually we cannot use ninja in parallel, because it writes to the same .ninja_log etc..
    # However, this seems to only be a problem when we rebuild later. Thus, we only build in
    # parallel when not doing incremental builds (which is *much* faster).
    if incremental:
        for t in tasks:
            t.get(incremental)
    else:
        build_all(tasks, incremental)

    # build bbl separately as it has a different out_path
    if 'riscv64' in isas:
        t = BuildTask(name='build/riscv-pk',
                      in_path='.',
                      out_path='build/riscv-pk',
                      cache_dir=cache_dir,
                      cmd='M3_ISA=riscv64 M3_BUILD=bench ./b mkbbl -n',
                      shell=True)
        t.get(incremental)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='This is the M³ builder.')
    parser.add_argument('--target', nargs='+', default=['gem5', 'hw22', 'hw23'])
    parser.add_argument('--isa', nargs='+', default=['riscv32', 'riscv64', 'x86_64'])
    parser.add_argument('--build', nargs='+', default=['debug', 'release'])
    parser.add_argument('command')
    args = parser.parse_args()

    cache_dir = '/cache'
    if args.command == 'prepare':
        prepare(targets=args.target, isas=args.isa,
                cache_dir=cache_dir, incremental=False)
    elif args.command == 'build':
        build(targets=args.target, isas=args.isa, builds=args.build,
              cache_dir=cache_dir, incremental=False)
    else:
        print("Unknown command '{}'".format(args.command))
        sys.exit(1)
