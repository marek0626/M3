def build(gen, env):
    if 'riscv' in env['ISA']:
        # use our own start file (Entry.S)
        env = env.clone()
        env['LINKFLAGS'] += ['-nostartfiles']

        dir = 'riscv' if 'riscv' in env['ISA'] else env['ISA']
        entry_file = 'src/arch/' + dir + '/Entry.S'
        entry = env.asm(gen, out=entry_file[:-2] + '.o', ins=[entry_file])

        lib = env.static_lib(gen, out='unimux', ins=[entry])
        env.install_as(gen, env['LIBDIR'] + '/libunimux.a', lib)

        env.m3_rust_lib(gen, features=["unimux/" + env['TGT']])
