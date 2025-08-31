#!/usr/bin/env python3

import os
import argparse
import subprocess

from pathlib import Path
from typing import List

# disk geometry (512 * 31 * 63 ~= 1 mb)
secsize = 512
hdheads = 31
hdtracksecs = 63


def create_disk(image: Path, fs: Path, parts: List[int], offset: int) -> None:
    if len(parts) == 0:
        exit("Please provide at least one partition")
    if len(parts) > 4:
        exit("Sorry, the maximum number of partitions is currently 4")

    # determine size of disk
    totalmb = 0
    for p in parts:
        totalmb += int(p)
    hdcyl = totalmb

    # create image and copy file system into partition
    subprocess.call(["dd", "if=" + str(fs), "of=" + str(image),
                     "bs=512", "seek=" + str(offset)])

    # zero beginning
    subprocess.call(["dd", "if=/dev/zero", "of=" + str(image),
                     "bs=512", "conv=notrunc", "count=" + str(offset)])

    tmpfile = subprocess.check_output("mktemp").rstrip()
    lodev = create_loop(image)
    # build command file for fdisk
    with open(tmpfile, "w") as f:
        i = 1
        for p in parts:
            # n = new partition, p = primary, partition number, default offset
            f.write('n\np\n' + str(i) + '\n\n')
            # the last partition gets the remaining sectors
            if i == len(parts):
                f.write('\n')
            # all others get all sectors up to the following partition
            else:
                f.write(str(block_offset(parts, offset, i) * 2 - 1) + '\n')
            # make first partition bootable
            if i == 1:
                f.write('\na\n')
            i += 1
        # write partitions to disk
        f.write('w\n')

    # create partitions with fdisk
    with open(tmpfile, "r") as fin:
        proc = subprocess.Popen(
            ["sudo", "fdisk", "-u", "-C", str(hdcyl), "-S", str(hdheads), lodev], stdin=fin
        )
        proc.wait()
    free_loop(lodev)

    # remove temp file
    subprocess.call(["rm", "-Rf", tmpfile])


def mb_to_blocks(mb: int) -> int:
    """determines the number of blocks for `mb` MB"""
    return int((mb * hdheads * hdtracksecs) / 2)


def block_offset(parts: List[int], secoffset: int, no: int) -> int:
    """determines the block offset for partition `no` in `parts`"""
    i = 0
    off = secoffset / 2
    for p in parts:
        if i == no:
            return int(off)
        off += mb_to_blocks(p)
        i += 1
    assert False, "<no> out of bounds"


def create_loop(image: Path, offset: int = 0) -> str:
    """creates a free loop device for `image`, starting at `offset`"""
    lodev = subprocess.check_output(["sudo", "losetup", "-f"], text=True).rstrip()
    subprocess.call(["sudo", "losetup", "-o", str(offset), lodev, image])
    return lodev


def free_loop(lodev: str) -> None:
    """frees loop device `lodev`"""
    # sometimes the resource is still busy, so try it a few times
    i = 0
    while i < 10 and subprocess.call(["sudo", "losetup", "-d", lodev]) != 0:
        i += 1


def run_fdisk(image: Path) -> None:
    """runs fdisk for `image`"""
    lodev = create_loop(image)
    hdcyl = int(os.path.getsize(image) / (1024 * 1024))
    subprocess.call(["sudo", "fdisk", "-u", "-C", str(hdcyl), "-S", str(hdheads), lodev])
    free_loop(lodev)


def run_parted(image: Path) -> None:
    """runs parted for `image`"""
    lodev = create_loop(image)
    subprocess.call(["sudo", "parted", lodev, "print"])
    free_loop(lodev)


def create(args: argparse.Namespace) -> None:
    size = subprocess.check_output(["stat", "--format=%s", args.fs]).rstrip()
    create_disk(args.disk, args.fs, [int(int(size) / (1024 * 1024))], 2048)


def fdisk(args: argparse.Namespace) -> None:
    run_fdisk(args.disk)


def parted(args: argparse.Namespace) -> None:
    run_parted(args.disk)


# argument handling
parser = argparse.ArgumentParser(description='This is a tool for creating disk images with'
                                 + ' specified partitions. Additionally, you can mount partitions'
                                 + ' and analyze the disk with fdisk and parted.')
subparsers = parser.add_subparsers(
    title='subcommands', description='valid subcommands', help='additional help'
)

parser_create = subparsers.add_parser('create', description='Writes a new disk image to <diskimage>'
                                      + ' with the file system <fs>.')
parser_create.add_argument('disk', metavar='<diskimage>')
parser_create.add_argument('fs', metavar='<fs>')
parser_create.set_defaults(func=create)

parser_fdisk = subparsers.add_parser('fdisk', description='Runs fdisk for <diskimage>.')
parser_fdisk.add_argument('disk', metavar='<diskimage>')
parser_fdisk.set_defaults(func=fdisk)

parser_parted = subparsers.add_parser('parted', description='Runs parted for <diskimage>.')
parser_parted.add_argument('disk', metavar='<diskimage>')
parser_parted.set_defaults(func=parted)

args = parser.parse_args()
try:
    func = args.func
except AttributeError:
    parser.error("too few arguments")
func(args)
