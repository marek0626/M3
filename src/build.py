dirs = [
    "toolchain",  # generate linker scripts first
    "libs",       # libs and unimux afterwards (can be used by others)
    "unimux",
    "kernel",
    "apps",
    "rot",
    "tilemux",
    "server",
    "fs",         # generate the file systems last
]


def build(gen, env):
    for d in dirs:
        env.sub_build(gen, d)
