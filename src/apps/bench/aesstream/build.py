def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env.m3_rust_exe(
            gen,
            out='aesstream',
            libs=['isr-nostackswitch', 'unimux'],
            varAddr=False,
            ldscript='isr',
        )

