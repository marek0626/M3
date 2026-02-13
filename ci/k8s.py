#!/usr/bin/env python3

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import json

from pathlib import Path
from typing import Any, Optional

ROOT = Path(__file__).resolve().parent
NS = "os"
OUT_DIR = ROOT / "out"

POD_NAME = "m3-ci"
IMG_NAME = "m3-ci"

CACHE_NAME = "m3-ci-cache"
CACHE_SIZE = "1000Gi"

RESULTS_NAME = "m3-ci-results"
RESULTS_SIZE = "100Gi"


def run(*cmd: str,
        cwd: Optional[Path] = None,
        input: Optional[str] = None,
        capture: Optional[int] = None,
        check: bool = True) -> subprocess.CompletedProcess[Any]:
    result = subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        input=input,
        check=check,
        text=True,
        stdout=capture,
        stderr=capture,
    )
    return result


def apply_yaml(yaml: str) -> None:
    run(
        "kubectl", "apply", "-f", "-",
        input=yaml,
    )


def create_storage(name: str, size: str) -> None:
    yaml = run(
        "sh", str(ROOT / "config" / "storage.sh"), name, size,
        capture=subprocess.PIPE,
    ).stdout
    apply_yaml(yaml)


def create_image(name: str) -> None:
    # create image with podman (run from $ROOT)
    build_cmd = [
        "podman", "build",
        "-t", name,
        ".",
    ]
    run(*build_cmd, cwd=ROOT)

    # tag & push
    remote_tag = (
        f"registry.adbi.barkhauseninstitut.org/os/code/m3/m3/{name}:latest"
    )
    run("podman", "image", "tag", f"{name}:latest", remote_tag)
    run("podman", "image", "push", remote_tag)


def create_pod(name: str, image: str) -> None:
    # ``kubectl get pod`` returns 0 if the pod exists, non‑zero otherwise.
    try:
        run("kubectl", "get", "pod", "-n", NS, name, capture=subprocess.PIPE)
        return                     # pod already present
    except subprocess.CalledProcessError:
        pass

    pod_yaml = run(
        "sh", str(ROOT / "config" / "pod.sh"), name, image,
        capture=subprocess.PIPE,
    ).stdout
    apply_yaml(pod_yaml)

    # Wait for the pod to become ready (timeout 5 min)
    run(
        "kubectl", "wait", "-n", NS,
        "--for=condition=ready",
        "--timeout=5m",
        f"pod/{name}",
    )


def remove_pod(name: str) -> None:
    run(
        "kubectl", "delete", "-n", NS, f"pod/{name}", "--now",
        capture=subprocess.DEVNULL,
    )
    run(
        "kubectl", "wait", "-n", NS,
        "--for=delete",
        "--timeout=5m",
        f"pod/{name}",
    )


def exec_shell(name: str) -> None:
    run(
        "kubectl", "exec", "-n", NS, "-ti", name, "--", "bash",
        check=False,   # propagate the remote shell's exit status
    )


def debug_test(test_dir: str) -> None:
    fd, tmp_name = tempfile.mkstemp(suffix=".sh")
    os.close(fd)  # ensure that the file is not open anymore
    tmp_path = Path(tmp_name)

    try:
        run(
            "kubectl", "cp", "-n", NS,
            f"{POD_NAME}:{test_dir}/run.sh",
            str(tmp_path),
        )
        # make executable
        tmp_path.chmod(tmp_path.stat().st_mode | 0o111)
        run(str(tmp_path))
    finally:
        try:
            tmp_path.unlink()
        except FileNotFoundError:
            pass


def deploy_secrets(kubecfg: str) -> None:
    success = True
    success &= deploy_kube_secret(kubecfg)
    success &= deploy_glab_secret(kubecfg)
    if not success:
        exit(1)


def deploy_kube_secret(kubecfg: str) -> bool:
    try:
        run("kubectl", "delete", "secret", "m3-ci-kubecfg", check=False)
        result = run("kubectl", "create", "secret", "generic", "m3-ci-kubecfg",
                     f"--from-file=config={kubecfg}")
    except subprocess.CalledProcessError as e:
        print(e, file=sys.stderr)
        return False
    return True


def deploy_glab_secret(kubecfg: str) -> bool:
    try:
        # this assumes that the secure file if found on the first page of output
        result = run("glab", "securefile", "list", cwd=ROOT,
                     capture=subprocess.PIPE)
        secure_files = json.loads(result.stdout)
        kubecfg_files = [f for f in secure_files if f["name"] == "m3-ci-kubecfg"]
        assert len(kubecfg_files) <= 1
        if len(kubecfg_files) == 1:
            kubecfg_id = kubecfg_files[0]["id"]
            run("glab", "securefile", "remove", "--yes", "--", str(kubecfg_id), cwd=ROOT)

        # this needs to run in the git repository so that glab knows the correct remote repository
        # to store the secret in
        kubecfg = str(Path(kubecfg).absolute())
        run("glab", "securefile", "create", "m3-ci-kubecfg", "--", kubecfg, cwd=ROOT)
    except subprocess.CalledProcessError as e:
        print(e, file=sys.stderr)
        return False
    return True


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Helper script for CI image / pod handling."
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    # img <gitlab-user> <gitlab-pw>
    img_parser = subparsers.add_parser(
        "img",
        help="Create a new image.",
    )

    # run
    subparsers.add_parser(
        "run",
        help="Start the CI pod and drop into an interactive shell.",
    )

    # debug <test-dir>
    debug_parser = subparsers.add_parser(
        "debug",
        help="Download run.sh from the CI pod and execute it.",
    )
    debug_parser.add_argument(
        "test_dir",
        help="Path inside the CI pod that contains run.sh (in /cache/results)",
    )

    # rm
    subparsers.add_parser("rm", help="Remove the CI pod.")

    # secrets <kubecfg>
    secrets_parser = subparsers.add_parser(
        "secrets",
        help="Deploy secrets.",
        epilog=(
            "This command requires the command line tools kube and glab to be installed and "
            "configured/authorized."
        )
    )
    secrets_parser.add_argument(
        "kubecfg",
        help="Kubernetes configuration file (with token) like found in ~/.kube/config",
    )

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args(sys.argv[1:])

    if args.command == "img":
        create_image(IMG_NAME)
    elif args.command == "run":
        create_storage(CACHE_NAME, CACHE_SIZE)
        create_storage(RESULTS_NAME, RESULTS_SIZE)
        create_pod(POD_NAME, IMG_NAME)
        exec_shell(POD_NAME)
    elif args.command == "debug":
        create_pod(POD_NAME, IMG_NAME)
        debug_test(args.test_dir)
    elif args.command == "rm":
        remove_pod(POD_NAME)
    elif args.command == "secrets":
        deploy_secrets(args.kubecfg)


if __name__ == "__main__":
    main()
