import argparse
import os
import shutil
import subprocess
import sys

CACHE_DIR = '/cache'
REPO = 'https://ci:gldt-Y6KXy-AKNDZ8uUb8PnY3@gitlab.barkhauseninstitut.org/os/code/M3/M3.git'


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
                 cmd, shell=False, cleanup=""):
        self.name = name
        self.in_path = in_path
        self.out_path = out_path
        self.cmd = cmd
        self.shell = shell
        self.cleanup = cleanup

    def hash(self):
        return get_hash(self.in_path)

    def cache_path(self):
        return '{}/{}/{}'.format(CACHE_DIR, self.name, self.hash())

    def needs_rebuild(self):
        return not os.path.exists(self.cache_path())

    def get(self, no_build=False):
        rebuild = self.needs_rebuild()
        if rebuild:
            if no_build:
                return
            log, proc = self.start()
            proc.wait()
            log.close()
            if proc.returncode != 0:
                print('{}: exited with status {}'.format(self.name, proc.returncode))
                sys.exit(1)
        self.finish(rebuild)

    def start(self):
        mkdir(os.path.dirname('{}/logs/{}'.format(CACHE_DIR, self.name)))
        logfile = '{}/logs/{}.log'.format(CACHE_DIR, self.name)
        log = open(logfile, 'w+')
        print("{}: rebuilding... (log in {})".format(self.out_path, logfile))
        sys.stdout.flush()
        proc = subprocess.Popen(self.cmd, shell=self.shell,
                                stdout=log, stderr=log)
        return (log, proc)

    def finish(self, rebuild: bool):
        print("{}: ready".format(self.out_path))
        sys.stdout.flush()
        if rebuild:
            if len(self.cleanup) > 0:
                subprocess.run(self.cleanup, shell=True)
            mkdir(os.path.dirname(self.cache_path()))
            subprocess.run(['mv', self.out_path, self.cache_path()])
        if os.path.islink(self.out_path):
            os.unlink(self.out_path)
        elif os.path.isdir(self.out_path):
            shutil.rmtree(self.out_path)
        if os.path.split(self.out_path)[0] != '':
            mkdir(os.path.dirname(self.out_path))
        os.symlink(self.cache_path(), self.out_path)


def build_all(tasks: [BuildTask], no_build: bool):
    running = []
    for t in tasks:
        if t.needs_rebuild():
            if not no_build:
                running.append((t, t.start()))
        else:
            t.finish(False)
    for (task, (log, proc)) in running:
        proc.wait()
        log.close()
        if proc.returncode != 0:
            print('{}: exited with status {}'.format(task.name, proc.returncode))
            sys.exit(1)
        task.finish(True)


parser = argparse.ArgumentParser(description='This is the M³ builder.')
parser.add_argument('command')
parser.add_argument('-c', '--commit', default='origin/virteps')
parser.add_argument('-n', '--no-build', action='store_true')
args = parser.parse_args()

if args.command == 'prepare':
    subprocess.run(['git', 'clone', REPO])
    os.chdir('M3')
    subprocess.run(['git', 'checkout', args.commit])

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
        t.get(args.no_build)

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
                      cleanup='rm -rf build/cross-{}/build'.format(isa),
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

    build_all(tasks, args.no_build)

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

    build_all(tasks, args.no_build)

    if not args.no_build:
        # start with a clean result directory
        shutil.rmtree('{}/result'.format(CACHE_DIR), ignore_errors=True)

        # collect files and directories to copy
        files = [('platform/gem5', 'submodules/gem5', '.')]
        for isa in ['RISCV', 'X86']:
            files.append(('platform/gem5', 'build/gem5', isa + '/gem5.opt'))
        for build in ['debug', 'bench']:
            for isa in ['riscv32', 'riscv64', 'x86_64']:
                dir = 'm3-gem5-{}-{}'.format(isa, build)
                files.append(('.', 'build/' + dir, 'bin/stripped'))
                files.append(('.', 'build/' + dir, 'src/fs'))
                files.append(('.', 'build/' + dir, 'toolsbin'))

        # now copy results into /cache/result
        for (in_path, name, path) in files:
            hash = get_hash(in_path)
            src = '{}/{}/{}/{}'.format(CACHE_DIR, name, hash, path)
            dst = '{}/result/{}/{}/{}'.format(CACHE_DIR, name, hash, path)
            print("Copying '{}'...".format(src))
            sys.stdout.flush()
            if os.path.isfile(src):
                mkdir(os.path.dirname(dst))
                shutil.copy2(src, dst)
            else:
                shutil.copytree(src, dst, symlinks=True)
else:
    print("Unknown command '{}'".format(args.command))
