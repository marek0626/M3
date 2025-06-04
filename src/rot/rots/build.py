from ninjapie import BuildPath


def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env = env.clone()
        env['CRGENV']['M3_ROTS'] = '1'
        env['LINKFLAGS'] += ['-nostartfiles']
        o = env.m3_rust_exe(
            gen,
            out='rots',
            libs=['kecacc-xkcp', 'isr-nostackswitch', 'unimux'],
            ldscript='unimux',
            varAddr=False,
        )
        bin = env.objcopy(gen, BuildPath.with_file_ext(env, o, 'bin'), o, type='binary')
        env.install(gen, env['BUILDDIR'] + '/rotbin', bin)
