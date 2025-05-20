def build(gen, env):
    if 'riscv' in env['ISA']:
        dir = 'riscv' if 'riscv' in env['ISA'] else env['ISA']
        for sf in [True, False]:
            env = env.new(env['ISA'], sf)
            # use our own start file (Entry.S)
            env['LINKFLAGS'] += ['-nostartfiles']

            entry_file = 'src/arch/' + dir + '/Entry.S'
            entry = env.asm(gen, out=entry_file[:-2] + '-' + str(sf) + '.o', ins=[entry_file])

            lib = env.static_lib(gen, out='unimux-' + str(sf), ins=[entry])
            env.install_as(gen, env['LIBDIR'] + '/libunimux.a', lib)

        env.m3_rust_lib(gen)
