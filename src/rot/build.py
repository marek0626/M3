from ninjapie import BuildPath


def build(gen, env):
    if env['ISA'] != 'riscv64' and env['ISA'] != 'riscv32':
        # (because of riscv-rt and the RISC-V specific assembly code)
        return
    if env['TGT'] == 'hw22':
        # (rosa uses the TCU TileDesc register, which is not available on hw22)
        return

    # the RoT always runs on a riscv32 core on the hardware platform
    if env['TGT'] == 'hw23':
        isa = 'riscv32'
    else:
        isa = env['ISA']

    # we want soft-float
    env = env.new(isa, True)

    # FIXME: The RoT cannot be built in debug mode at the moment.
    # There are two issues:
    #   1. The RoT layers have fixed memory regions that are too small for
    #      the unoptimized debug builds.
    #   2. Unused code that makes use of heap allocations is not properly
    #      discarded in debug builds, so the RoT layers without heap run into
    #      linker errors (e.g. "undefined hidden symbol: __rdl_alloc").
    if env['BUILD'] == 'debug':
        env['BUILD'] = 'release'
        env['CRGFLAGS'] += ['--release']

    # riscv64imac-unknown-none-elf works too and is a standard Rust target
    if env['ISA'] == 'riscv64':
        env['TRIPLE'] = 'riscv64imc-unknown-none-elf'
    else:
        env['TRIPLE'] = 'riscv32imc-unknown-none-elf'

    # Non-standard target, need to build the standard library ourselves
    env['CRGFLAGS'] += ['-Z build-std=core,alloc']
    # Can be used to completely remove panic messages from the binary
    # env['CRGFLAGS'] += ['-Z build-std-features=panic_immediate_abort']

    for o in ["brom", "blau", "rosa", "rots"]:
        old_cwd = env.cur_dir
        env._cwd.path += '/' + o
        build_stage(gen, env, o)
        env._cwd.path = old_cwd


def build_stage(gen, env, out):
    env = env.clone()

    env['CPPFLAGS'] += ['-D__' + out + '__']
    ldconf = env.cpp(gen, out='ld.conf', input='../ld.conf')
    env.install_as(gen, env['LDDIR'] + '/ld-' + out + '.conf', ldconf)

    if out == 'rots':
        env['CRGENV']['M3_ROTS'] = '1'
        libs = ['kecacc-xkcp', 'isr-nostackswitch', 'unimux']
        env['LINKFLAGS'] += ['-nostartfiles']
    else:
        libs = []

    if env['TGT'] == 'hw23':
        env['RUSTCFLAGS'] += ['-C', 'opt-level=z']

    exe = env.m3_rust_exe(
        gen,
        out=out,
        libs=libs,
        ldscript=out,
        dir=None,
        varAddr=False,
    )
    if out == 'brom':
        env.install(gen, outdir=env['BUILDDIR'] + '/rotbin', input=exe)
    bin = env.objcopy(gen, BuildPath.with_file_ext(env, exe, 'bin'), exe, type='binary')
    env.install(gen, env['BUILDDIR'] + '/rotbin', bin)
