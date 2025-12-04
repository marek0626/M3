def build(gen, env):
    if 'riscv' in env['ISA']:
        # note that this needs to go first in order for us to add the unimux files to the
        # dependenicies of unimuxheap.
        env.m3_rust_lib(gen)

        env.sub_build(gen, 'unimuxheap')

        for isa in ['riscv32', 'riscv64']:
            for sf in [True, False]:
                env = env.new(isa, sf)
                # use our own start file (Entry.S)
                env['LINKFLAGS'] += ['-nostartfiles']

                entry_file = 'src/arch/riscv/Entry.S'
                suffix = '-' + isa + '-' + str(sf)

                entry = env.asm(gen, out=entry_file[:-2] + suffix + '.o', ins=[entry_file])
                lib = env.static_lib(gen, out='unimux' + suffix, ins=[entry])
                env.install_as(gen, env['LIBDIR'] + '/libunimux.a', lib)
