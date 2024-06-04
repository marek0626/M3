#!/bin/bash
cat <<EOF
{
    "apiVersion": "v1",
    "kind": "PersistentVolumeClaim",
    "metadata": {
        "name": "m3-build-cache",
        "namespace": "os"
    },
    "spec": {
        "accessModes": ["ReadWriteOnce"],
        "resources": {
            "requests": {
                "storage": "200Gi"
            }
        },
        "storageClassName": "ceph-block",
        "volumeMode": "Filesystem"
    }
}
EOF
