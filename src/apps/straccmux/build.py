def build(gen, env):
    # there is not enough memory for the debug version
    # we also can only use it on the hw platform (RISC-V only) and on gem5 with RISC-V
    if env['BUILD'] == 'debug' or not env['ISA'].startswith('riscv'):
        return

    env = env.new('riscv32', True)
    env['RUSTCFLAGS'] += ['-C', 'opt-level=z']

    ldconf = env.cpp(gen, out='ld.conf', input='ld.conf')
    env.install_as(gen, env['LDDIR'] + '/ld-straccmux.conf', ldconf)

    env.m3_rust_exe(
        gen,
        out='straccmux',
        ldscript='straccmux',
        dir=None,
        varAddr=False,
    )
