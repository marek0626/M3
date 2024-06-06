def build(gen, env):
    env.m3_exe(gen, out='cppalgobenchs', ins=env.glob(gen, '*.cc'))
