#!/bin/bash

ns="os"

set -e

create_cache_stor() {
    sh ./config/storage.sh m3-build-cache 200Gi | kubectl apply -f -
}

create_perm_stor() {
    sh ./config/storage.sh m3-perm 100Gi | kubectl apply -f -
}

create_image() {
    name="$1"
    podman build --build-arg "KUBECFG=out/kubecfg" -t "$name" .
    podman image tag "$name:latest" "registry.hpc.barkhauseninstitut.org/$ns/$name:latest"
    podman image push "registry.hpc.barkhauseninstitut.org/$ns/$name:latest"
}

create_pod() {
    name="$1"
    image="$2"
    mount="$3"
    volume="$4"
    buildpod=$(sh ./config/pod.sh "$name" "$image" "$mount" "$volume")
    echo "$buildpod" | kubectl apply -f -
    kubectl wait -n "$ns" --for=condition=ready --timeout=5m "pod/$name"
}

remove_pod() {
    name="$1"
    kubectl delete -n "$ns" "pod/$name" --now || true
    kubectl wait -n "$ns" --for=delete --timeout=5m "pod/$name"
}

exec_shell() {
    name="$1"
    kubectl exec -n "$ns" -ti "$name" -- bash
}

cp -f "$HOME/.kube/config" out/kubecfg
trap 'rm -f out/kubecfg 2>/dev/null' EXIT ERR INT TERM

case "$1" in
    ciimg)
        create_image m3-ci
        ;;
    ci)
        create_cache_stor
        remove_pod m3-ci
        create_image m3-ci
        create_pod m3-ci m3-ci /cache m3-build-cache
        exec_shell m3-ci
        ;;
    test)
        create_perm_stor
        remove_pod m3-test
        create_image m3-test
        create_pod m3-test m3-test /code m3-perm
        exec_shell m3-test
        ;;
    *)
        echo "Usage: $0 (ci|test)" >&2
        exit 1
esac
