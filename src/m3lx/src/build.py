
dirs = [
    'base',
    'lxclient',
    'm3',
    'simplebench',
]

def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
    env.cargo_ws(gen)
