def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env = env.clone()
        env['LINKFLAGS'] += ['-nostartfiles']
        env.m3_rust_exe(
            gen,
            out='aesstream',
            libs=['isr-nostackswitch', 'unimux', 'unimuxentry'],
            varAddr=False,
            ldscript='isr',
        )
