def build(gen, env):
    env.m3_exe(gen, out='cppvfsbenchs', ins=env.glob(gen, '*.cc'))
