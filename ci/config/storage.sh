#!/bin/bash
name="$1"
size="$2"
cat <<EOF
{
    "apiVersion": "v1",
    "kind": "PersistentVolumeClaim",
    "metadata": {
        "name": "$name",
        "namespace": "os"
    },
    "spec": {
        "accessModes": ["ReadWriteOnce"],
        "resources": {
            "requests": {
                "storage": "$size"
            }
        },
        "storageClassName": "ceph-block",
        "volumeMode": "Filesystem"
    }
}
EOF
