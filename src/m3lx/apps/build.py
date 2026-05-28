dirs = [
    "lxcppbenchs",
    "lxrustbenchs",
    "starter",
    "tcutest",
    "proxy",
]


def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
