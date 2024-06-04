#!/bin/bash

ns="os"

set -ex

create_image() {
    name="$1"
    cache="$2"
    podman build --build-arg "CACHE=$2" -t "$name" .
    podman image tag "$name:latest" "registry.hpc.barkhauseninstitut.org/$ns/$name:latest"
    podman image push "registry.hpc.barkhauseninstitut.org/$ns/$name:latest"
}

remove_pods() {
    pods=("pod/m3-build")
    for isa in riscv32 riscv64 x86-64; do
        pods=("${pods[@]}" "pod/m3-test-$isa")
    done
    kubectl delete -n "$ns" "${pods[@]}" --now || true
    kubectl wait -n "$ns" --for=delete --timeout=5m "${pods[@]}"
}

create_cache() {
    cache=$(sh ./config/cache.sh)
    echo "$cache" | kubectl apply -f -
}

create_build_con() {
    remove_pods
    create_image m3-build empty

    buildpod=$(sh ./config/build-pod.sh m3-build m3-build)
    echo "$buildpod" | kubectl apply -f -
    kubectl wait -n "$ns" --for=condition=ready --timeout=5m pod m3-build
}

create_test_con() {
    remove_pods
    create_image m3-test builds
}

exec_build_con() {
    kubectl exec -n "$ns" -t m3-build -- ./build.sh
    rm -rf builds
    kubectl cp "$ns"/m3-build:/cache/result builds
    chmod +x builds/build/m3-gem5-*/*/toolsbin/*
    chmod +x builds/build/gem5/*/*/gem5.opt
    kubectl delete -n "$ns" pod m3-build --now
}

exec_test_con() {
    mkdir -p logs
    for isa in riscv32 riscv64 x86_64; do
        nameisa="$(echo $isa | tr _ -)"
        testpod=$(sh ./config/build-pod.sh "m3-test-$nameisa" m3-test)
        echo "$testpod" | kubectl apply -f -
        kubectl wait -n "$ns" --for=condition=ready --timeout=5m pod "m3-test-$nameisa"
        echo "Starting m3-test-$isa..."
        kubectl exec -n "$ns" -t "m3-test-$nameisa" -- \
            ./build.sh -n --isa $isa /results &> logs/m3-test-$isa.log &
    done
    wait

    for isa in riscv32 riscv64 x86_64; do
        nameisa="$(echo $isa | tr _ -)"
        echo "Retrieving results from m3-test-$isa..."
        kubectl cp "$ns/m3-test-$nameisa:/results" results
        kubectl delete -n "$ns" pod "m3-test-$nameisa" --now
    done
}

exec_shell() {
    name="$1"
    if [[ "$name" == m3-test* ]]; then
        image="m3-test"
    else
        image="m3-build"
    fi
    pod=$(sh ./config/build-pod.sh "$name" "$image")
    echo "$pod" | kubectl apply -f -
    kubectl wait -n "$ns" --for=condition=ready --timeout=5m pod "$name"
    kubectl exec -n "$ns" -ti "$name" -- bash
}
