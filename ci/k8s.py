#!/usr/bin/env python3

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

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
        cwd: Path | None = None,
        input: str | None = None,
        capture: int | None = None,
        check: bool = True) -> subprocess.CompletedProcess:
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


def apply_yaml(yaml: str):
    run(
        "kubectl", "apply", "-f", "-",
        input=yaml,
    )


def create_storage(name: str, size: str):
    yaml = run(
        "sh", str(ROOT / "config" / "storage.sh"), name, size,
        capture=subprocess.PIPE,
    ).stdout
    apply_yaml(yaml)


def create_image(name: str, gitlab_user: str, gitlab_pw: str):
    user_file = OUT_DIR / "user"
    pw_file = OUT_DIR / "pw"

    try:
        # create tmp files for gitlab user/pw
        user_file.write_text(gitlab_user + "\n")
        pw_file.write_text(gitlab_pw + "\n")

        # create image with podman (run from $ROOT)
        build_cmd = [
            "podman", "build",
            "--build-arg", "KUBECFG=out/kubecfg",
            "--build-arg", "GITLAB_USER=out/user",
            "--build-arg", "GITLAB_PW=out/pw",
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

    finally:
        for p in (user_file, pw_file):
            try:
                p.unlink()
            except FileNotFoundError:
                pass


def create_pod(name: str, image: str):
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


def remove_pod(name: str):
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


def exec_shell(name: str):
    run(
        "kubectl", "exec", "-n", NS, "-ti", name, "--", "bash",
        check=False,   # propagate the remote shell's exit status
    )


def debug_test(test_dir: str):
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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Helper script for CI image / pod handling."
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    # img <gitlab-user> <gitlab-pw>
    img_parser = subparsers.add_parser(
        "img",
        help="Create a new image (requires GitLab credentials).",
    )
    img_parser.add_argument("gitlab_user", help="GitLab user name")
    img_parser.add_argument("gitlab_pw", help="GitLab password / token")

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

    return parser


def main():
    parser = build_parser()
    args = parser.parse_args(sys.argv[1:])

    # Prepare the temporary output directory and copy the kubeconfig (once)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    src_cfg = Path.home() / ".kube" / "config"
    dst_cfg = OUT_DIR / "kubecfg"
    shutil.copy2(src_cfg, dst_cfg)

    if args.command == "img":
        create_image(IMG_NAME, args.gitlab_user, args.gitlab_pw)
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


if __name__ == "__main__":
    main()
