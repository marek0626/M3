def build(gen, env):
    lib = env.m3_rust_staticlib(gen, out='unimuxheap')
    # remove the dummy _Unwind_Resume symbol as the application linking against that might want to
    # define that (for C++, for example).
    lib = env.objcopy(gen, out='unimuxheap', input=lib, flags=['--strip-symbol=_Unwind_Resume'])
    env.install_as(gen, env['LIBDIR'] + '/libunimuxheap.a', lib)
