from ninjapie import BuildPath


def build(gen, env):
    if env['ISA'].startswith('riscv'):
        o = env.m3_rust_tee_exe(
            gen, out='rots', libs=['kecacc-xkcp'], dir=None, ldscript='unimux'
        )
        bin = env.objcopy(gen, BuildPath.with_file_ext(env, o, 'bin'), o, type='binary')
        env.install(gen, env['BUILDDIR'] + '/rotbin', bin)
