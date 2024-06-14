#!/usr/bin/env bash

if [ $# -ne 1 ]; then
    echo "Usage: $0 <dir>"
fi

dir="$1"

if [ -d "$dir" ]; then
    kubectl exec -n os -t m3-ci-web -- sh -c "rm -rf /web/*";
    for f in "$dir"/*; do
        kubectl cp -n os "$f" m3-ci-web:/web;
    done
fi
