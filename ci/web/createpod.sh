#!/usr/bin/env bash
name="m3-ci-web"
image="m3-ci-web"
kubectl apply -f - <<EOF
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
        "resources": {
          "requests": {
            "memory": "1000Mi",
            "cpu": "128m"
          },
          "limits": {
            "memory": "1000Mi",
            "cpu": "128m"
          }
        },
        "volumeMounts": [
          {
            "mountPath": "/web",
            "name": "web"
          }
        ]
      }
    ],
    "volumes": [
      {
        "name": "web",
        "persistentVolumeClaim": {
          "claimName": "m3-ci-web"
        }
      }
    ]
  }
}
EOF

