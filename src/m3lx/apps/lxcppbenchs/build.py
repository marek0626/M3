def build(gen, env):
    files = ['lxcppbenchs.cc']
    m3benchs = '../../../apps/bench/cppbenchs/benchs'
    for b in ['bregfile', 'bactivity']:
        files += [m3benchs + '/' + b + '.cc']
    env.lx_exe(gen, out='lxcppbenchs', ins=files)
