#!/bin/bash
name="$1"
image="$2"
mount="$3"
volume="$4"
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
            "mountPath": "$mount",
            "name": "volume"
          }
        ]
      }
    ],
    "volumes": [
      {
        "name": "volume",
        "persistentVolumeClaim": {
          "claimName": "$volume"
        }
      }
    ]
  }
}
EOF
