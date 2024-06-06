def build(gen, env):
    files = ['lxcppbenchs.cc']
    m3benchs = '../../../apps/bench/cppbenchs'
    for b in ['vfs/bregfile', 'misc/bactivity']:
        files += [m3benchs + '/' + b + '.cc']
    env.lx_exe(gen, out='lxcppbenchs', ins=files)
