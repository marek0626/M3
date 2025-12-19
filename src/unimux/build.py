def build(gen, env):
    if 'riscv' in env['ISA']:
        env.m3_rust_lib(gen)

        lib = env.m3_rust_staticlib(gen, out='unimux')
        env.install_as(gen, env['LIBDIR'] + '/libunimux.a', lib)

        for isa in ['riscv32', 'riscv64']:
            for sf in [True, False]:
                env = env.new(isa, sf)
                # use our own start file (Entry.S)
                env['LINKFLAGS'] += ['-nostartfiles']

                entry_file = 'src/arch/riscv/Entry.S'
                suffix = '-' + isa + '-' + str(sf)

                entry = env.asm(gen, out=entry_file[:-2] + suffix + '.o', ins=[entry_file])
                lib = env.static_lib(gen, out='unimux' + suffix, ins=[entry])
                env.install_as(gen, env['LIBDIR'] + '/libunimuxentry.a', lib)
