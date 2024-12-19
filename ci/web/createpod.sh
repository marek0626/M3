#!/usr/bin/env bash
name="m3-ci-web"
image="m3-ci-web"
kubectl apply -f - <<EOF
{
  "apiVersion": "apps/v1",
  "kind": "StatefulSet",
  "metadata": {
    "name": "$name",
    "namespace": "os"
  },
  "spec": {
    "serviceName": "m3-ci-web",
    "replicas": 1,
    "selector": {
      "matchLabels": {
        "pod": "m3-ci-web",
        "user": "nils"
      }
    },
    "template": {
      "metadata": {
        "labels": {
          "pod": "m3-ci-web",
          "user": "nils"
        }
      },
      "spec": {
        "restartPolicy": "Always",
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
  }
}
EOF
