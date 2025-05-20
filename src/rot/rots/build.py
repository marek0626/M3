dirs = ["app"]


def build(gen, env):
    env['LINKFLAGS'] += ['-nostartfiles']

    env.add_rust_features()

    for d in dirs:
        env.sub_build(gen, d)
