dirs = [
    'accel',
    'base',
    'heap',
    'heapsimple',
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
    'mux',
]


def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
