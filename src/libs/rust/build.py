dirs = [
    'accel',
    'base',
    'heap',
    'isr',
    'lang',
    'm3',
    'm3core',
    'm3files',
    'paging',
    'pci',
    'resmng',
    'thread',
    'vtermcli',
    'pipecli',
]


def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
