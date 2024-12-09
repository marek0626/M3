import argparse
import os
import sys

if not os.path.isfile('ci/builder.py'):
    print("Please invoke this script from the M3 root directory")
    sys.exit(1)

sys.path.append(os.path.realpath('ci'))  # NOQA
import builder

parser = argparse.ArgumentParser(description='Garbage collects entries in given directory.')
parser.add_argument('-m', '--max', default=10, type=int)
parser.add_argument('path')
args = parser.parse_args()

builder.gc_dir(args.path, args.max)
