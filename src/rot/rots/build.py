from ninjapie import SourcePath


def build(gen, env):
    # ROT is not valid outside riscv ISAs.
    if env['ISA'].startswith('riscv'):
        env = env.clone()

        env['LINKFLAGS'] += ['-nostartfiles']

        # rots also depends on unimux
        deps = env.rust_deps()
        deps += [SourcePath.new(env, '../../unimux/Cargo.toml')]
        deps += env.glob(gen, '../../unimux/**/*.rs')
        env['CRGFLAGS'] += ['--target', env['TRIPLE']]
        env.add_rust_features()

        lib = env.rust_exe(gen, out='librots.a', deps=deps)
        env.install(gen, env['LIBDIR'], lib)

        o = env.m3_rust_exe(
            gen,
            out='rots',
            libs=['kecacc-xkcp', 'isr-nostackswitch', 'unimux'],
            dir=None,
            startup=None,
            ldscript='unimux',
            varAddr=False,
            cargo_ws=False,
        )

        bin = env.objcopy(gen, env['BINDIR'] + '/rots.bin', o, type='binary')
        env.install(gen, env['BUILDDIR'] + '/rotbin', bin)
