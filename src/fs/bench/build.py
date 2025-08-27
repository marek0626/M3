def build(gen, env):
    blocks = 64 * 1024 if env['BUILD'] == 'debug' else 32 * 1024
    env.build_fs(gen, out='bench.img', dir='.', blocks=blocks, inodes=4096)
