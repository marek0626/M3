#!/bin/bash

ns="os"

set -e

create_cache_stor() {
    sh ./config/storage.sh m3-ci-cache 500Gi | kubectl apply -f -
}

create_image() {
    name="$1"
    user="$2"
    pw="$3"

    # create tmp files for gitlab user/pw
    echo "$user" > out/user
    echo "$pw" > out/pw
    trap 'rm -f out/user out/pw 2>/dev/null' EXIT ERR INT TERM

    # create image
    podman build \
        --build-arg "KUBECFG=out/kubecfg" \
        --build-arg "GITLAB_USER=out/user" \
        --build-arg "GITLAB_PW=out/pw" \
        -t "$name" .

    # tag and push to repo
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

mkdir -p out
cp -f "$HOME/.kube/config" out/kubecfg
trap 'rm -f out/kubecfg 2>/dev/null' EXIT ERR INT TERM

usage() {
    echo "Usage: $1 (img ...|run|rm)"
    echo ""
    echo "Commands:"
    echo "  img <gitlab-user> <gitlab-pw> - create new image"
    echo "  run                           - run shell in CI pod"
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
        if ! kubectl get pod -n "$ns" m3-ci &>/dev/null; then
            create_pod m3-ci m3-ci /cache m3-ci-cache
        fi
        exec_shell m3-ci
        ;;
    rm)
        remove_pod m3-ci
        ;;
    *)
        usage "$0"
esac
