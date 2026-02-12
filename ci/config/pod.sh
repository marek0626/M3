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
    "imagePullSecrets": [{
      "name": "m3-ci-pull"
    }],
    "containers": [
      {
        "name": "$name",
        "image": "registry.adbi.barkhauseninstitut.org/os/code/m3/m3/$image:latest",
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
          },
          {
            "mountPath": "/root/.kube",
            "name": "kubecfg",
            "readOnly": true
          },
          {
            "mountPath": "/root/.gitlab",
            "name": "gitlab",
            "readOnly": true
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
      },
      {
        "name": "kubecfg",
        "secret": {
          "secretName": "m3-ci-kubecfg",
          "items": [
            {
              "key": "config",
              "path": "config"
            }
          ],
          "defaultMode": 256
        }
      },
      {
        "name": "gitlab",
        "secret": {
          "secretName": "m3-ci-gitlab",
          "items": [
            {
              "key": "password",
              "path": "pw"
            }
          ],
          "defaultMode": 256
        }
      }
    ]
  }
}
EOF
