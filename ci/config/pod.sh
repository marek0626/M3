#!/usr/bin/env bash
name="$1"
image="$2"
cat <<EOF
{
  "apiVersion": "v1",
  "kind": "Pod",
  "metadata": {
    "name": "$name",
    "namespace": "os"
  },
  "spec": {
    "restartPolicy": "Never",
    "containers": [
      {
        "name": "$name",
        "image": "registry.hpc.barkhauseninstitut.org/os/$image:latest",
        "command": ["/usr/bin/sleep", "Infinity"],
        "resources": {
          "requests": {
            "memory": "$((32 * 1024))Mi",
            "cpu": "16000m"
          },
          "limits": {
            "memory": "$((256 * 1024))Mi",
            "cpu": "96000m"
          }
        },
        "volumeMounts": [
          {
            "mountPath": "/cache",
            "name": "cache"
          },
          {
            "mountPath": "/results",
            "name": "results"
          }
        ]
      }
    ],
    "volumes": [
      {
        "name": "cache",
        "persistentVolumeClaim": {
          "claimName": "m3-ci-cache"
        }
      },
      {
        "name": "results",
        "persistentVolumeClaim": {
          "claimName": "m3-ci-results"
        }
      }
    ]
  }
}
EOF
