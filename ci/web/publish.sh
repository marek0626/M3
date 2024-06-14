#!/usr/bin/env bash

if [ -d /reports ]; then
    kubectl exec -n os -t m3-ci-web -- sh -c "rm -rf /web/*";
    for f in /reports/*; do
        kubectl cp -n os "$f" m3-ci-web:/web;
    done
fi
