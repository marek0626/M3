from ninjapie import BuildPath


def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env = env.clone()
        env['LINKFLAGS'] += ['-nostartfiles']

        crt0_obj = BuildPath(env['LIBDIR'] + '/crtbegin.o')
        crtn_obj = BuildPath(env['LIBDIR'] + '/crtend.o')
        ins = [crt0_obj, 'teetestcpp.cc', crtn_obj]

        env.m3_exe(
            gen,
            ins=ins,
            out='teetestcpp',
            libs=['isr-nostackswitch', 'unimux', 'unimuxheap'],
            varAddr=False,
            ldscript='isr',
        )
