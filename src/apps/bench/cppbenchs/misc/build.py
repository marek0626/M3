def build(gen, env):
    env.m3_exe(gen, out='cppmiscbenchs', ins=env.glob(gen, '*.cc'))
