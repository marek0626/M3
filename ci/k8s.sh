#!/usr/bin/env bash

root=$(dirname "$0")
ns="os"

set -e

create_cache_stor() {
    sh "$root/config/storage.sh" m3-ci-cache 1000Gi | kubectl apply -f -
}

create_results_stor() {
    sh "$root/config/storage.sh" m3-ci-results 100Gi | kubectl apply -f -
}

create_image() {
    name="$1"
    user="$2"
    pw="$3"

    # create tmp files for gitlab user/pw
    echo "$user" >"$root/out/user"
    echo "$pw" >"$root/out/pw"
    trap 'rm -f "$root/out/user" "$root/out/pw" 2>/dev/null' EXIT ERR INT TERM

    # create image
    (cd "$root" && podman build \
        --build-arg "KUBECFG=out/kubecfg" \
        --build-arg "GITLAB_USER=out/user" \
        --build-arg "GITLAB_PW=out/pw" \
        -t "$name" .)

    # tag and push to repo
    podman image tag "$name:latest" "registry.adbi.barkhauseninstitut.org/os/code/m3/m3/$name:latest"
    podman image push "registry.adbi.barkhauseninstitut.org/os/code/m3/m3/$name:latest"
}

create_pod() {
    name="$1"
    image="$2"
    if ! kubectl get pod -n "$ns" "$name" &>/dev/null; then
        buildpod=$(sh "$root/config/pod.sh" "$name" "$image")
        echo "$buildpod" | kubectl apply -f -
        kubectl wait -n "$ns" --for=condition=ready --timeout=5m "pod/$name"
    fi
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

debug_test() {
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT ERR INT TERM
    kubectl cp -n "$ns" "m3-ci:$1/run.sh" "$tmp"
    chmod +x "$tmp"
    "$tmp"
}

mkdir -p "$root/out"
cp -f "$HOME/.kube/config" "$root/out/kubecfg"
trap 'rm -f "$root/out/kubecfg" 2>/dev/null' EXIT ERR INT TERM

usage() {
    echo "Usage: $1 (img ...|run|debug <test-dir>|rm)"
    echo ""
    echo "Commands:"
    echo "  img <gitlab-user> <gitlab-pw> - create new image"
    echo "  run                           - run shell in CI pod"
    echo "  debug <test-dir>              - download run.sh from test directory and execute it"
    echo "  rm                            - remove CI pod"
    exit 1
}

case "$1" in
    img)
        if [ $# != 3 ]; then
            usage "$0"
        fi
        create_image m3-ci "$2" "$3"
        ;;
    run)
        create_cache_stor
        create_results_stor
        create_pod m3-ci m3-ci
        exec_shell m3-ci
        ;;
    debug)
        if [ $# != 2 ]; then
            usage "$0"
        fi
        create_pod m3-ci m3-ci
        debug_test "$2"
        ;;
    rm)
        remove_pod m3-ci
        ;;
    *)
        usage "$0"
        ;;
esac
