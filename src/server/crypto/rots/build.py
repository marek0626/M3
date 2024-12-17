def build(gen, env):
    # ROT is not valid outside riscv ISAs.
    if env['ISA'].startswith('riscv'):
        env = env.clone()

        env['LINKFLAGS'] += ['-nostartfiles']

        env.m3_rust_exe(
            gen,
            out='rots',
            libs=['kecacc-xkcp', 'isr-nostackswitch', 'unimux'],
            dir=None,
            startup=None,
            ldscript='isr',
            varAddr=False,
        )
