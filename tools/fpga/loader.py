import os
import sys

from elftools.elf.elffile import ELFFile

import memory
import pm
from tcu import EP, MemEP, Flags, TCUExtReg
from tile import Tile, TileType

import utils

DRAM_SIZE = 2 * 1024 * 1024 * 1024
DRAM_OFF = 0x10000000

KENV_ADDR = 0
KENV_SIZE = 4 * 1024
SERIAL_ADDR = KENV_ADDR + KENV_SIZE
SERIAL_SIZE = 4 * 1024
PMP_ADDR = SERIAL_ADDR + SERIAL_SIZE


def dram_tile(dram: memory) -> int:
    if dram.name == "DRAM1":
        return 8
    return 9


class Loader:
    def __init__(self, tcu_version: (int, int, int), pmp_size: int, vm: bool):
        self.tcu_version = tcu_version
        self.pmp_size = pmp_size
        self.vm = vm

    def init(self, tiles: list[pm], drams: list[memory], dram: memory, kernels: list[str],
             rot_layers: list[str], mods: list[str], logflags: str) -> list[int]:
        if rot_layers is None:
            ktiles = [i for i in range(0, len(tiles)) if tiles[i].type == TileType.ROCKET]
            # load kernel info into DRAM
            if self.vm:
                mods_addr = PMP_ADDR + (len(kernels) * self.pmp_size)
            else:
                mods_addr = PMP_ADDR + (len(tiles) * self.pmp_size)
            self._load_kernel_info(tiles, drams, dram, mods, mods_addr)
        else:
            ktiles = [i for i in range(0, len(tiles))
                      if tiles[i].type == TileType.ACC and "ACC-Hash" in tiles[i].desc.attrs()]
            assert len(ktiles) > 0, "RoT tile (with SHA-3 accelerator) not found"
            # load RoT info into memory
            self._load_rot_info(dram, ktiles[0], rot_layers, mods)

        if len(kernels) > len(ktiles):
            msg = "Insufficient kernel tiles (need {}, have {})".format(len(kernels),
                                                                        len(ktiles))
            raise ValueError(msg)

        # init all tiles
        loaded_tiles = ktiles[0:len(kernels)]
        for i, tile in enumerate(tiles, 0):
            self._init_tile(dram, tile, i, i in loaded_tiles)

        # load kernels on tiles (only for Rocket cores)
        for i, pargs in enumerate(kernels, 0):
            self._load_prog(tiles, drams, dram, ktiles[i], pargs.split(' '), logflags)

        return loaded_tiles

    def start(self, tiles: list[pm], loaded: list[int], debug: int):
        # start kernel tiles
        debug_tile = len(tiles) if debug is None else debug
        for i, tile in enumerate(tiles, 0):
            if i == debug_tile or i not in loaded:
                continue
            print("Starting tile {}: {}".format(i, tiles[i].name))
            sys.stdout.flush()
            if tiles[i].type == TileType.ROCKET:
                # start core (via interrupt 0)
                tiles[i].inst.rocket_start()
            elif tiles[i].type == TileType.ACC:
                tiles[i].inst.asm_enable()
                tiles[i].inst.acc_enable()

    def _load_rot_info(self, dram: memory, rot_tile: int, rot_layers: list[str], mods: list[str]):
        cfg_off = PMP_ADDR + rot_tile * self.pmp_size

        # load next layers into memory
        rot_off = cfg_off + 0x1000
        layers = []
        for layer in rot_layers[1:]:
            path = os.path.basename(layer)
            size = os.path.getsize(layer)
            self._write_file(dram, path, rot_off)
            layers.append((rot_off - cfg_off, size))
            rot_off = (rot_off + size + 4096 - 1) & ~(4096 - 1)

        # write config into memory
        cfg_off = PMP_ADDR + rot_tile * self.pmp_size
        # brom
        utils.write_u64(dram, cfg_off + 0 * 8, 0x42726f6d43666701)        # brom.magic
        brom = layers[0]
        utils.write_u64(dram, cfg_off + 1 * 8, brom[0] | brom[1] << 32)   # brom.next_layer
        # blau
        utils.write_u64(dram, cfg_off + 2 * 8, 0x426c617543666701)        # blau.magic
        blau = layers[1]
        utils.write_u64(dram, cfg_off + 3 * 8, blau[0] | blau[1] << 32)   # blau.next_layer
        # rosa
        utils.write_u64(dram, cfg_off + 4 * 8, 0x526f736143666702)        # rosa.magic
        utils.write_u64(dram, cfg_off + 5 * 8, 16 * 1024 * 1024)          # rosa.kernel_mem_size
        # utils.write_u64(dram, cfg_off + 6 * 8, 0)                       # rosa.kernel_eps_pages
        kmod = next(m for m in mods if m.startswith("kernel"))
        (kmod_name, _) = kmod.split("=")
        utils.write_str(dram, kmod_name, cfg_off + 6 * 8 + 1)
        # mods
        mods_desc = cfg_off + 6 * 8 + 48
        mods_addr = rot_off
        for m in mods:
            mod_size = self._add_mod(dram, mods_addr, m, mods_desc)
            mods_addr = (mods_addr + mod_size + 4096 - 1) & ~(4096 - 1)
            mods_desc += 80
        # null-terminate mods array
        utils.write_u64(dram, mods_desc + 8, 0)
        rosa = layers[2]
        next_off = cfg_off + 6 * 8 + 48 + 25 * 80
        utils.write_u64(dram, next_off, rosa[0] | rosa[1] << 32)          # rosa.next_layer

    def _init_tile(self, dram: memory, tile: pm, tile_idx: int, loaded: bool):
        # reset TCU (clear command log and reset registers except FEATURES and EPs)
        tile.tcu_reset(resetBits=0xF)

        # enable instruction trace for all Rocket tiles (doesn't cost anything)
        if tile.type == TileType.ROCKET:
            tile.inst.rocket_enableTrace()
            tile.inst.start()
        elif tile.type == TileType.ACC:
            tile.inst.asm_enableTrace()

        # set features: privileged, vm, ctxsw
        tile.tcu_set_features(1, self.vm, 1)

        # invalidate all EPs
        for ep in range(0, 127):
            tile.tcu_set_ep(ep, EP.invalid())

        # init PMP EP (for loaded tiles or if SPM should be emulated)
        if loaded or not self.vm:
            mem_begin = PMP_ADDR + tile_idx * self.pmp_size
            mem_size = self.pmp_size

            # install first PMP EP
            pmp_ep = MemEP()
            pmp_ep.set_chip(dram.nocid[0])
            pmp_ep.set_tile(dram.nocid[1])
            pmp_ep.set_act(0xFFFF)
            pmp_ep.set_flags(Flags.READ | Flags.WRITE)
            pmp_ep.set_addr(mem_begin)
            pmp_ep.set_size(mem_size)
            tile.tcu_set_ep(0, pmp_ep)

    def _load_kernel_info(self, tiles: list[pm], drams: list[memory], dram: memory,
                          mods: list[str], mods_addr: int):
        # boot info
        kenv_off = KENV_ADDR
        utils.write_u64(dram, kenv_off + 0 * 8, len(mods))                      # mod_count
        utils.write_u64(dram, kenv_off + 1 * 8, len(tiles) + 1)                 # tile_count
        utils.write_u64(dram, kenv_off + 2 * 8, len(drams))                     # mem_count
        utils.write_u64(dram, kenv_off + 3 * 8, 0)                              # serv_count
        kenv_off += 8 * 4

        # mods
        for m in mods:
            mod_size = self._add_mod(dram, mods_addr, m, kenv_off)
            mods_addr = (mods_addr + mod_size + 4096 - 1) & ~(4096 - 1)
            kenv_off += 80

        # tile descriptors
        for x in range(0, len(tiles)):
            utils.write_u64(dram, kenv_off, self._tile_desc(tiles, x))          # PM
            kenv_off += 8
        utils.write_u64(dram, kenv_off, self._tile_desc(tiles, len(tiles)))     # dram1
        kenv_off += 8

        # mems
        mem_start = mods_addr
        for d in drams:
            addr = utils.glob_addr(d.nocid[1], 0)
            size = DRAM_SIZE
            if dram.nocid == d.nocid:
                addr += mem_start
                size -= mem_start
            utils.write_u64(dram, kenv_off + 0, addr)                           # addr
            utils.write_u64(dram, kenv_off + 8, size)                           # size
            kenv_off += 16

    def _load_prog(self, tiles: list[pm], drams: list[memory], dram: memory,
                   tile_idx: int, args: list[str], logflags: str):
        pm = tiles[tile_idx]

        # start core
        env_off = 0x1000
        if pm.type == TileType.ROCKET:
            entry = 0x10004000
            mem_tile = dram
            mem_off = PMP_ADDR + tile_idx * self.pmp_size
            mem_begin = mem_off - DRAM_OFF
            env = 0x10001000
        else:
            entry = 0x4000
            mem_tile = pm
            mem_off = 0
            mem_begin = 0
            env = 0x1000

        print("%s: loading %s..." % (pm.name, args[0]))
        sys.stdout.flush()

        # verify entrypoint, because inject a jump instruction below that jumps to that address
        with open(args[0], 'rb') as f:
            elf = ELFFile(f)
            if elf.header['e_entry'] != entry:
                sys.exit("error: {} has entry {:#x}, not {:#x}.".format(
                    args[0], elf.header['e_entry'], entry))

        # load ELF file
        mem_tile.mem.write_elf(args[0], mem_begin)
        sys.stdout.flush()

        desc = self._tile_desc(tiles, tile_idx)
        kenv = utils.glob_addr(dram_tile(mem_tile), KENV_ADDR) if tile_idx == 0 else 0

        # write arguments and env vars
        argv = env + 0x400
        args_end = env + 0x800
        envp = self._write_args(mem_tile, args, argv, mem_begin, args_end)
        if logflags:
            self._write_args(mem_tile, ["LOG=" + logflags], envp, mem_begin, args_end)
        else:
            envp = 0

        # init environment
        mem_env = env_off + mem_off
        jump = 0x6f + (entry & 0x0FFFFFFF)
        utils.write_u64(mem_tile, mem_off, jump)            # j _start
        utils.write_u64(mem_tile, mem_env + 0, 1)           # platform = HW
        utils.write_u64(mem_tile, mem_env + 8, tile_idx)    # chip, tile
        utils.write_u64(mem_tile, mem_env + 16, desc)       # tile_desc
        utils.write_u64(mem_tile, mem_env + 24, len(args))  # argc
        utils.write_u64(mem_tile, mem_env + 32, argv)       # argv
        utils.write_u64(mem_tile, mem_env + 40, envp)       # envp
        utils.write_u64(mem_tile, mem_env + 48, kenv)       # kenv
        utils.write_u64(mem_tile, mem_env + 56, len(tiles) + len(drams))  # raw tile count
        # tile ids
        env_off = 64
        for tile in tiles:
            utils.write_u64(mem_tile, mem_env + env_off, tile.nocid[0] << 8 | tile.nocid[1])
            env_off += 8
        for d in drams:
            utils.write_u64(mem_tile, mem_env + env_off, d.nocid[0] << 8 | d.nocid[1])
            env_off += 8

        sys.stdout.flush()

    def _add_mod(self, dram: memory, addr: int, mod: str, offset: int) -> int:
        (name, path) = mod.split('=')
        path = os.path.basename(path)
        size = os.path.getsize(path)
        utils.write_u64(dram, offset + 0x0, utils.glob_addr(dram_tile(dram), addr))
        utils.write_u64(dram, offset + 0x8, size)
        utils.write_str(dram, name, offset + 16)
        self._write_file(dram, path, addr)
        return size

    def _write_file(self, dram: memory, file: str, offset: int):
        print("%s: loading %s with %u bytes to %#x" %
              (dram.name, file, os.path.getsize(file), offset))
        sys.stdout.flush()

        with open(file, "rb") as f:
            content = f.read()
        dram.mem.write_bytes_checked(offset, content, True)

    def _write_args(self, mem: Tile, args: list[str], argv: int,
                    mem_begin: int, args_end: int) -> int:
        argc = len(args)
        args_addr = argv + (argc + 1) * 8
        for (idx, a) in enumerate(args, 0):
            # write pointer
            utils.write_u64(mem, argv + mem_begin + idx * 8, args_addr)
            # write string
            utils.write_str(mem, a, args_addr + mem_begin)
            args_addr += (len(a) + 1 + 7) & ~7
            if args_addr > args_end:
                sys.exit("Not enough space for arguments")
        # null termination
        utils.write_u64(mem, argv + mem_begin + argc * 8, 0)
        return args_addr

    def _tile_desc(self, tiles: list[pm], tile_idx: int):
        if tile_idx >= len(tiles):
            # mem size | TileAttr::IMEM | TileType::MEM
            return (DRAM_SIZE >> 12) << 28 | ((1 << 4) << 11) | 1

        tile = tiles[tile_idx]
        desc = tile.mem[tile.tcu.ext_reg_addr(TCUExtReg.TILE_DESC)]

        if not self.vm and (desc & ((1 << 4) << 11)) == 0:
            # mem size | TileAttr::IMEM
            desc |= ((self.pmp_size >> 12) << 28) | ((1 << 4) << 11)
        if self.tcu_version[0] < 3:
            desc |= (1 << 5) << 11  # IEPS

        return desc
