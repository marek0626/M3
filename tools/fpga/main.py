#!/usr/bin/env python3

import argparse
import traceback
from time import sleep, time
import threading
import os
import sys

import fpga_top
from noc import NoCmonitor
from fpga_utils import FPGA_Error
from tile import TileType

import loader
import term


timeout_ev = threading.Event()
started_ev = threading.Event()


class TimeoutThread(threading.Thread):
    def __init__(self, timeout):
        super(TimeoutThread, self).__init__()
        self.daemon = True
        self.timeout = timeout
        self.start()

    def run(self):
        end = int(time()) + self.timeout
        while True:
            now = int(time())
            if now >= end:
                break
            sleep(end - now)
        print("Execution timed out after {} seconds".format(self.timeout))
        sys.stdout.flush()
        timeout_ev.set()
        if not started_ev.is_set():
            os._exit(1)


def run_loop(fpga_inst, serial, timeout_ev):
    if serial is not None:
        terminal = term.LxTerm(serial)
    elif not sys.stdin.isatty():
        terminal = term.NullTerm()
    else:
        terminal = term.TCUTerm(fpga_inst.dram1, fpga_inst.nocif)

    # write in binary to stdout (we get individual bytes from Linux, for example)
    fdout = os.fdopen(sys.stdout.fileno(), "wb", closefd=False)

    timed_out = False
    try:
        while True:
            # check for timeout
            if timeout_ev.is_set():
                timed_out = True
                break

            # check if there is input to pass to the FPGA
            if terminal.should_stop():
                # force-extract logs on ctrl+]
                timed_out = True
                break

            # check for output
            try:
                bytes = fpga_inst.nocif.receive_bytes(timeout_ns=10_000_000)
            except Exception:
                continue

            fdout.write(bytes)
            fdout.flush()

            # stop when we see the shutdown message from the M³ kernel
            try:
                msg = bytes.decode()
                if "Shutting down" in msg:
                    break
            except Exception:
                pass
    except KeyboardInterrupt:
        timed_out = True

    terminal.cleanup()

    return timed_out


def extract_tcu_stats(tile, no: int):
    if tile.tcu_version()[0] < 4:
        try:
            drops = tile.tcu_drop_flit_count()
            errors = tile.tcu_error_flit_count()
            print("PM{}: TCU dropped/error flits: {}/{}".format(no, drops, errors))
        except Exception as e:
            print("PM{}: unable to read number of TCU dropped flits: {}".format(no, e))


def extract_tcu_log(tile, no: int):
    print("PM{}: reading TCU log...".format(no))
    sys.stdout.flush()
    try:
        tile.tcu_print_log('log/pm' + str(no) + '-tcu-cmds.log')
    except Exception as e:
        print("PM{}: unable to read TCU log: {}".format(no, e))
        print("PM{}: resetting TCU and reading all logs...".format(no))
        sys.stdout.flush()
        tile.tcu_reset()
        try:
            tile.tcu_print_log('log/pm' + str(no) + '-tcu-cmds.log', all=True)
        except Exception:
            pass


def extract_instr_trace(tile, no: int):
    if tile.type == TileType.ROCKET:
        trace = []
        for traceNum in range(2):
            try:
                trace += tile.inst.rocket_getTrace(all=False, traceNum=traceNum)
            except Exception as e:
                print("PM{}: unable to read instruction trace: {}".format(no, e))
                print("PM{}: resetting TCU and reading all logs...".format(no))
                sys.stdout.flush()
                tile.tcu_reset()
                try:
                    trace += tile.inst.rocket_getTrace(all=True, traceNum=traceNum)
                except Exception:
                    pass
        if len(trace) > 0:
            tile.inst.rocket_printCombinedTrace('log/pm' + str(no) + '-instrs.log', trace)
    elif tile.type == TileType.ACC:
        try:
            tile.inst.asm_printTrace('log/pm' + str(no) + '-instrs.log')
        except Exception as e:
            print("PM{}: unable to read instruction trace: {}".format(no, e))
            print("PM{}: resetting TCU and reading all logs...".format(no))
            sys.stdout.flush()
            tile.tcu_reset()
            try:
                tile.inst.asm_printTrace('log/pm' + str(no) + '-instrs.log', all=True)
            except Exception:
                pass
        tile.inst.asm_disable()


def stop_tiles(fpga_inst, version, extract, timed_out):
    print("Stopping all tiles...")
    for i, tile in enumerate(fpga_inst.pmTiles, 0):
        # if tile is locked, unlock it first
        if version == 4 and tile.tcu_get_lock():
            tile.tcu_unlock()
        if extract:
            extract_tcu_stats(tile, i)
            if timed_out:
                extract_tcu_log(tile, i)
            extract_instr_trace(tile, i)

        if tile.type == TileType.ROCKET:
            tile.inst.stop()
        elif tile.type == TileType.ACC:
            tile.inst.asm_disable()
            tile.inst.acc_disable()
    # read logs in DRAM tiles
    if extract and timed_out:
        extract_tcu_log(fpga_inst.dram1, 8)
        extract_tcu_log(fpga_inst.dram2, 9)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--fpga', type=int)
    parser.add_argument('--version', type=int)
    parser.add_argument('--reset', action='store_true')
    parser.add_argument('--debug', type=int)
    parser.add_argument('--tile', action='append')
    parser.add_argument('--mod', action='append')
    parser.add_argument('--rotlayer', action='append')
    parser.add_argument('--vm', action='store_true')
    parser.add_argument('--serial')
    parser.add_argument('--logflags')
    parser.add_argument('--timeout', type=int)
    args = parser.parse_args()

    NoCmonitor()
    if args.timeout is not None:
        TimeoutThread(args.timeout)

    # connect to FPGA
    fpga_inst = fpga_top.FPGA_TOP(args.version, args.fpga, args.reset)

    # disable NoC ARQ for program upload
    if args.version < 4:
        fpga_inst.set_arq_enable(False)

    # stop all tiles
    stop_tiles(fpga_inst, args.version, False, False)

    # check TCU versions
    for tile in fpga_inst.pmTiles:
        version = tile.tcu_version()
        if version[0] != args.version:
            print("Tile %s has TCU major version %d, but expected %d" %
                  (tile.name, version[0], args.version))
            return

    mods = [] if args.mod is None else args.mod
    pmp_size = 16 * 1024 * 1024 if args.vm else 64 * 1024 * 1024

    ld = loader.Loader(version, pmp_size, args.vm)

    # disable TCU logging during loading
    fpga_inst.tcu_log_enable(False)

    drams = [fpga_inst.dram1, fpga_inst.dram2]
    dram = drams[1] if args.rotlayer is not None else drams[0]
    loaded = ld.init(fpga_inst.pmTiles, drams, dram, args.tile,
                     args.rotlayer, mods, args.logflags)

    # enable NoC ARQ and TCU logging when cores are running
    if args.version < 4:
        fpga_inst.set_arq_enable(True)
    fpga_inst.tcu_log_enable(True)

    ld.start(fpga_inst.pmTiles, loaded, args.debug)

    # signal run.sh that everything has been loaded
    if args.debug is not None:
        ready = open('.ready', 'w')
        ready.write('1')
        ready.close()

    # wait for prints
    started_ev.set()

    timed_out = run_loop(fpga_inst, args.serial, timeout_ev)

    # disable NoC ARQ and TCU logging again for post-processing
    if args.version < 4:
        fpga_inst.set_arq_enable(False)
    fpga_inst.tcu_log_enable(False)

    stop_tiles(fpga_inst, args.version, True, timed_out)


try:
    main()
except FPGA_Error:
    sys.stdout.flush()
    traceback.print_exc()
except Exception:
    sys.stdout.flush()
    traceback.print_exc()
except KeyboardInterrupt:
    pass
