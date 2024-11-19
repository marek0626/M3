dirs = [
    'cshake',
    'kecacc',
    'kecacc-xkcp',
    'rot',
    'hex',
]


def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
