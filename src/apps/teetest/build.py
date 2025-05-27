def build(gen, env):
    if env['ISA'].startswith('riscv'):
        env.m3_rust_tee_exe(gen, out='teetest')
