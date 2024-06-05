#!/bin/bash

backend="k8s"

if [ "$1" = "-b" ]; then
    backend="$2"
    shift 2
fi

source "backend/${backend}.sh"

set -e

mkdir empty
trap 'rmdir empty 2>/dev/null' EXIT ERR INT TERM
cp -f "$HOME/.kube/config" kubecfg
trap 'rm -f kubecfg 2>/dev/null' EXIT ERR INT TERM

case "$1" in
    buildsh)
        create_cache
        create_build_con
        exec_shell "m3-build"
        ;;
    build)
        create_cache
        create_build_con
        exec_build_con
        ;;
    test)
        create_test_con
        exec_test_con
        ;;
    testsh)
        create_test_con
        exec_shell "m3-test-riscv32"
        ;;
    *)
        create_cache
        create_build_con
        exec_build_con
        create_test_con
        exec_test_con
esac
