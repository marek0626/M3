from ninjapie import Env, BuildPath


def build(gen, env):
    if env['ISA'] != 'riscv64' and env['ISA'] != 'riscv32':
        # (because of riscv-rt and the RISC-V specific assembly code)
        return
    if env['TGT'] == 'hw22':
        # (rosa uses the TCU TileDesc register, which is not available on hw22)
        return

    # we want soft-float
    env = env.new(env['ISA'], True)

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

    env.sub_build(gen, "rots")

    early = []
    # TODO note that cannot build multiple packages at once when specifying RUSTCFLAGS, because the
    # rustc command of cargo does not support that.
    for o in ["brom", "blau", "rosa"]:
        early.append(early_stage(env, gen, o))
    for o in early:
        env.install(gen, outdir=env['BINDIR'], input=o)
        # Install as raw binary as well for the RoT layers
        bin = env.objcopy(gen, BuildPath.with_file_ext(env, o, 'bin'), o, type='binary')
        env.install(gen, env['BUILDDIR'] + '/rotbin', bin)
    env.install(gen, outdir=env['BUILDDIR'] + '/rotbin', input=early[0])


def early_stage(env, gen, out):
    env = env.clone()

    ldconf = env.cpp(gen, out='memory-' + out + '.ld', input=out + '/memory.ld')
    env['RUSTCFLAGS'] += [
        "-C", "link-arg=-T" + os.path.abspath(ldconf),
        "-C", "link-arg=-Tlink.x",
        "-C", "link-arg=-T../gp.ld",
        # Avoid unneeded 4K alignment of sections
        "-C", "link-arg=-n",
        # Needed for backtraces, can be removed to save space
        "-C", "force-frame-pointers=y",
        "-Z", "llvm_module_flag=SmallDataLimit:u32:0:error",
        # The curve25519-dalek crate has two backends: one using 32-bit operations and one using
        # 64-bit operations. Normally this is auto-detected, but this does not seem to work for
        # custom targets/toolchains.
        "--cfg", '\'curve25519_dalek_bits="64"\'',
    ]

    env['CRGFLAGS'] += ['-p', out]
    env['CRGFLAGS'] += ['--target', env['TRIPLE']]

    deps = env.rust_deps_global()
    deps += env.glob(gen, out + '/**/Cargo.toml')
    deps += env.glob(gen, out + '/**/*.rs')
    deps += [ldconf]

    return Env.rust_exe(env, gen, out=out, deps=deps, dir='src')
