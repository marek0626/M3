def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env.m3_rust_exe(gen, out='teetest')
