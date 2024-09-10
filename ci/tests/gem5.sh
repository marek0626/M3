#!/usr/bin/env bash

set -e

root=$(dirname "$0")
inputdir="$root/../input"

source "$root/jobs.sh"

args=$(getopt -o ht:i:y:b: --long help,tests:,isas:,types:,bpes:,publish:,web: -n "$0" -- "$@")
eval set -- "$args"

usage() {
    echo "Usage: $0 [-t|--tests <tests>] [-i|--isas <isas>]"
    echo "          [-y|--types <types>] [-b|--bpe <bpe>]"
    echo "          [--publish <dir>] [--web <dir>] <results>"
    echo
    echo "  --tests <tests>: the list of tests to run, space separated."
    echo "  --isas <isas>  : the list of ISAs to use, space separated."
    echo "  --types <types>: the list of tile types (a, b, sh) to use, space separated."
    echo "  --bpes <bpes>  : the list of blocks per extents to use, space separated."
    echo "                   This is used for the file system images."
    echo "  --publish <dir>: Publish the results (on success) to <dir>."
    echo "  --web <dir>    : Generate CI results websites in <dir>."
    echo "  <results>      : the directory to store the results in."
    exit 1
}

isas="riscv32 riscv64 x86_64"
types="a b sh"
tests=""
bpes="32 64"
publish=""
web=""
while true; do
    case "$1" in
        -h | --help)
            usage
            ;;
        -t | --tests)
            tests="$2"
            shift 2
            ;;
        -i | --isas)
            isas="$2"
            shift 2
            ;;
        -y | --types)
            types="$2"
            shift 2
            ;;
        -b | --bpes)
            bpes="$2"
            shift 2
            ;;
        --publish)
            publish="$2"
            shift 2
            ;;
        --web)
            web="$2"
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

if [ "$1" = "" ]; then
    usage
fi
result="$1"

export M3_TARGET=gem5
if [ -z "$M3_GEM5_LOG" ]; then
    export M3_GEM5_LOG=Tcu,TcuRegWrite,TcuCmd,TcuConnector
fi
export M3_GEM5_CPUFREQ=3GHz M3_GEM5_MEMFREQ=1GHz
export M3_CORES=12
export M3_GEM5_CFG=$inputdir/test-config.py

run_bench() {
    export M3_ISA=$4
    export M3_TILETYPE=$3
    export ACCEL_NUM=0
    dirname=m3-tests-$2-$3-$4-$5
    bpe=$5
    bench=$2
    export M3_OUT=$1/$dirname
    mkdir -p "$M3_OUT"

    bootprefix=""
    if [ "$3" = "sh" ]; then
        export M3_TILETYPE=b
        bootprefix="shared/"
    elif [[ "$bench" =~ "ycsb-bench" ]]; then
        bootprefix=""
    fi
    if [ "$5" = "64" ]; then
        export M3_GEM5_CPU=DerivO3CPU
    else
        export M3_GEM5_CPU=TimingSimpleCPU
    fi

    if [ "$bench" = "unittests" ] || [ "$bench" = "rust-algo-tests" ] || [ "$bench" = "rust-misc-tests" ] ||
        [ "$bench" = "rust-vfs-tests" ] || [ "$bench" = "rust-destr-tests" ] || [ "$bench" = "hello" ] ||
        [ "$bench" = "rust-net-tests" ] || [ "$bench" = "cpp-net-tests" ] || [ "$bench" = "facever" ] ||
        [ "$bench" = "hashmux-tests" ] || [ "$bench" = "msgchan" ] || [ "$bench" = "resmngtest" ] ||
        [ "$bench" = "standalone" ] || [ "$bench" = "vmtest" ] || [ "$bench" = "rust-sndrcv" ] ||
        [ "$bench" = "libctest" ] || [ "$bench" = "rust-std-test" ] || [ "$bench" = "filterchain" ] ||
        [ "$bench" = "parchksum" ] || [ "$bench" = "shell-nested" ] || [ "$bench" = "chantests" ]; then
        if [ -f "boot/${bootprefix}$bench.xml" ]; then
            cp "boot/${bootprefix}$bench.xml" "$M3_OUT/boot.gen.xml"
        else
            cp "boot/$bench.xml" "$M3_OUT/boot.gen.xml"
        fi
        if [ "$bench" = "hello" ]; then
            export M3_BUILD=debug
        elif [ "$bench" = "standalone" ]; then
            export M3_GEM5_CFG=config/spm.py M3_CORES=8
        fi
    elif [[ "$bench" == lx* ]]; then
        cp "boot/linux/${bench#lx}.xml" "$M3_OUT/boot.gen.xml"
    elif [ "$bench" = "rots-raser" ] || [ "$bench" = "rots-hello" ]; then
        export M3_GEM5_HDD=build/$M3_TARGET-$M3_ISA-$M3_BUILD/fsimgs-$bpe/default.img
        unset M3_GEM5_CFG
        cp "boot/$bench.xml" "$M3_OUT/boot.gen.xml"
    elif [ "$bench" = "disk-test" ]; then
        export M3_GEM5_HDD=$inputdir/test-hdd.img
        cp "boot/${bootprefix}$bench.xml" "$M3_OUT/boot.gen.xml"
    elif [ "$bench" = "abort-test" ]; then
        export M3_GEM5_CFG=config/aborttest.py
        cp boot/hello.xml "$M3_OUT/boot.gen.xml"
    else
        if [[ "$bench" =~ "bench" ]] || [[ "$bench" =~ "voiceassist" ]]; then
            if [ "$bench" = "hashmux-benchs" ]; then
                export M3_CORES=18
            fi
            cp "boot/${bootprefix}$bench.xml" "$M3_OUT/boot.gen.xml"
        elif [[ "$bench" =~ "_" ]]; then
            IFS='_' read -ra parts <<< "$bench"
            writer=${parts[0]}_${parts[1]}_${parts[0]}
            reader=${parts[0]}_${parts[1]}_${parts[1]}
            export M3_ARGS="-d -i 1 -r 4 -w 1 $writer $reader"
            "$inputdir/${bootprefix}bench-scale-pipe.cfg" > "$M3_OUT/boot.gen.xml"
        elif [[ "$bench" =~ "imgproc" ]]; then
            IFS='-' read -ra parts <<< "$bench"
            if [ "${parts[1]}" = "indir" ]; then
                export M3_ACCEL_TYPE="indir"
            else
                export M3_ACCEL_TYPE="copy"
            fi
            export M3_ACCEL_COUNT=$((parts[2] * 3))
            export M3_ARGS="-m ${parts[1]} -n ${parts[2]} -w 1 -r 4 /large.txt"
            "$inputdir/${bootprefix}imgproc.cfg" > "$M3_OUT/boot.gen.xml"
        else
            export M3_ARGS="-n 4 -t -d -u 1 $bench"
            "$inputdir/${bootprefix}fstrace.cfg" > "$M3_OUT/boot.gen.xml"
        fi
    fi

    # we always use the FS images generated below
    export M3_MOD_PATH=build/$M3_TARGET-$M3_ISA-$M3_BUILD/fsimgs-$bpe

    /bin/echo -e "\e[1mStarting $dirname\e[0m"
    jobs_started

    # set memory and time limits
    if [ "$M3_GEM5_CPU" = "DerivO3CPU" ]; then
        ulimit -v 12000000  # 12GB virt mem
        ulimit -t 2400      # 40min CPU time
    else
        ulimit -v 7000000   # 6GB virt mem
        ulimit -t 1500      # 25min CPU time
    fi

    if nice ./b run "$M3_OUT/boot.gen.xml" -n < /dev/null > /dev/null 2>&1 \
        && "$root/check_result.py" "$M3_OUT/log.txt" 2>/dev/null; then
        /bin/echo -e "\e[1mFinished $dirname:\e[0m \e[1;32mSUCCESS\e[0m"
        rm -f "$M3_OUT/.failed"
    else
        /bin/echo -e "\e[1mFinished $dirname:\e[0m \e[1;31mFAILED\e[0m"
        echo > "$M3_OUT/.failed"
    fi

    gzip -f "$M3_OUT/gem5.log"
}

if [ "$M3_LOG" != "" ]; then
    M3_BUILD=release
else
    M3_BUILD=bench
fi

# create FS images
for isa in $isas; do
    build=build/$M3_TARGET-$isa-$M3_BUILD
    for bpe in $bpes; do
        bmoddir=build/$M3_TARGET-$isa-$M3_BUILD/fsimgs-$bpe
        mkdir -p "$bmoddir"

        benchblks=$((64 * 1024))
        defblks=$((16 * 1024))
        "$build/toolsbin/mkm3fs" "$bmoddir/bench.img" "$build/src/fs/bench" $benchblks 4096 "$bpe"
        "$build/toolsbin/mkm3fs" "$bmoddir/default.img" "$build/src/fs/default" $defblks 512 "$bpe"
    done
done

jobs_init "$(nproc)"

all=""
all+=" lxrust-benchs lxcpp-benchs lxtcutest"
all+=" rust-net-tests cpp-net-tests rust-net-benchs cpp-net-benchs"
all+=" rust-algo-tests rust-destr-tests rust-misc-tests rust-vfs-tests"
all+=" rust-algo-benchs rust-misc-benchs rust-vfs-benchs"
all+=" cpp-algo-benchs cpp-misc-benchs cpp-vfs-benchs"
all+=" chantests"
all+=" unittests hashmux-benchs hashmux-tests resmngtest"
all+=" facever rots-raser rots-hello"
all+=" find tar untar sqlite leveldb sha256sum sort"
all+=" cat_awk cat_wc grep_awk grep_wc"
all+=" disk-test abort-test"
all+=" standalone libctest rust-std-test msgchan rust-sndrcv vmtest"
all+=" ycsb-bench-udp ycsb-bench-tcp"
all+=" voiceassist-udp voiceassist-tcp"
all+=" bench-shell shell-nested parchksum filterchain"
# only 1 chain with indirect, because otherwise we would need more than 16 EPs
all+=" imgproc-indir-1"
for num in 1 2 3 4; do
    all+=" imgproc-dir-$num"
done

if [ "$tests" = "" ]; then
    tests="$all"
fi

for test in $tests; do
    for isa in $isas; do
        for bpe in $bpes; do
            for type in $types; do
                # riscv32 does not support VM
                if [ "$isa" == "riscv32" ] && [ "$type" != "a" ]; then
                    continue;
                fi

                # standalone works only with SPM
                if [ "$test" = "standalone" ] && [ "$type" != "a" ]; then
                    continue;
                fi
                # rots-raser only works on riscv64
                if [ "$test" = "rots-raser" ] && [ "$isa" != "riscv64" ]; then
                    continue;
                fi
                if [ "$test" = "rots-hello" ] && [ "$isa" == "x86_64" ]; then
                    continue;
                fi
                # rust-sndrcv and vmtest don't run with SPM
                if { [ "$test" = "rust-sndrcv" ] || [ "$test" = "vmtest" ]; } && [ "$type" = "a" ]; then
                    continue;
                fi
                # m3lx runs only on riscv64 and has no shared version
                if [[ "$test" == lx* ]] && { [ "$isa" != "riscv64" ] || [ "$type" != "b" ]; }; then
                    continue;
                fi

                jobs_submit run_bench "$result" "$test" "$type" "$isa" "$bpe"
            done
        done
    done
done

jobs_wait

# count succeeded and failed tests
failed=0
success=0
for test in "$result"/*; do
    if [ -f "$test/.failed" ]; then
        failed=$((failed + 1))
    elif [ -d "$test" ]; then
        success=$((success + 1))
    fi
done

# publish results if we consider the run "successful"
if [ "$publish" != "" ]; then
    if [ $failed -eq 0 ] || [ "$(((100 * success) / failed))" -gt 90 ]; then
        # garbage collect results: remove the results where the commits are no longer reachable
        for d in "$publish"/*; do
            hash=${d:20}
            if [ ${#hash} -eq 40 ] &&
               [ "$(git branch --remotes "--contains=$hash" 2>/dev/null)" == "" ]; then
                echo "Removing '$d' as the commit is no longer reachable."
                rm -rf "$d"
            fi
        done

        # copy all log files to result directory (don't keep gem5 logs etc.)
        resdst="$publish/$(date -I)-$(git rev-parse HEAD)"
        mkdir "$resdst"
        rsync -am --include='log.txt' --include='*/' --exclude='*' "$result"/* "$resdst"
        # copy coverage results from host tests
        rsync -am "$result"/coverage/ "$resdst"/coverage/

        # generate website
        if [ "$web" != "" ]; then
            "$root/../web/generate.py" "$publish" "$web"
        fi
    fi
fi

# print summary
echo
if [ $failed -eq 0 ]; then
    /bin/echo -e "\e[1mSummary: \e[1;32m$success of $((success + failed)) succeeded.\e[0m"
else
    /bin/echo -e "\e[1mSummary: \e[1;31m$success of $((success + failed)) succeeded.\e[0m"
    # now run over all directories and show failed tests
    printf "\nThe following tests failed:\n"
    for test in "$result"/*; do
        if [ -f "$test/.failed" ]; then
            echo "$test:"
            "$root/check_result.py" "$test/log.txt" || true
            echo
        fi
    done
    exit 1
fi
