#!/usr/bin/env python3
"""Tag release publishing; only Python's standard library and Cargo/Git are used."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request


class ReleaseError(Exception):
    """A release invariant failed; downstream release jobs must not run."""


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def publish_order(metadata, version):
    members = set(metadata["workspace_members"])
    packages = {p["name"]: p for p in metadata["packages"] if p["id"] in members}
    selected = {n: p for n, p in packages.items() if p.get("publish") != []}
    if "edda" not in selected:
        raise ReleaseError("No publishable edda CLI package in workspace")
    for name, package in packages.items():
        if package["version"] != version:
            raise ReleaseError(f"Tag {version} disagrees with {name}={package['version']}")
        if name in selected and package.get("publish") not in (None, ["crates-io"]):
            raise ReleaseError(f"{name}: registry restrictions do not permit this release")
    dependencies = {}
    for name, package in selected.items():
        edges = set()
        for dependency in package["dependencies"]:
            # Cargo drops unversioned dev-dependencies when publishing. All
            # normal/build workspace edges, including target/optional ones,
            # must precede their reader. Versioned dev edges matter too.
            if dependency.get("kind") == "dev" and dependency.get("req") == "*":
                continue
            target = dependency["name"]
            if dependency.get("path") and target in packages:
                if target not in selected:
                    raise ReleaseError(f"{name} depends on unpublished workspace crate {target}")
                edges.add(target)
        dependencies[name] = edges
    # CLI last is an invariant, not an accident of alphabetic iteration.
    dependencies["edda"].update(set(selected) - {"edda"})
    result = []
    while dependencies:
        ready = sorted(n for n, edges in dependencies.items() if not edges)
        if not ready:
            raise ReleaseError("Workspace publish dependency cycle (or a crate depends on edda)")
        for name in ready:
            result.append(selected[name])
            del dependencies[name]
        for edges in dependencies.values():
            edges.difference_update(ready)
    return result


class Registry:
    def __init__(self, sha, root):
        self.sha = sha
        self.root = Path(root).resolve()

    def fetch(self, url, missing_ok=False):
        for attempt in range(4):
            try:
                request = urllib.request.Request(url, headers={"User-Agent": "edda-release-ci/GH648"})
                with urllib.request.urlopen(request, timeout=60) as response:
                    return response.read()
            except urllib.error.HTTPError as error:
                if error.code == 404 and missing_ok:
                    return None
                if error.code not in (429, 500, 502, 503, 504):
                    raise ReleaseError(f"Registry HTTP {error.code}: {url}") from error
            except (urllib.error.URLError, TimeoutError):
                pass
            if attempt < 3:
                time.sleep(2 ** attempt)
        raise ReleaseError(f"Registry unavailable after retries: {url}")

    def verified(self, package):
        name, version = package["name"], package["version"]
        raw = self.fetch(f"https://crates.io/api/v1/crates/{name}/{version}", missing_ok=True)
        if raw is None:
            return False
        record = json.loads(raw)["version"]
        if record.get("yanked") is not False:
            raise ReleaseError(f"{name}@{version} is yanked or has invalid registry metadata")
        if record.get("num") != version or record.get("crate") != name:
            raise ReleaseError(f"Registry returned the wrong identity for {name}@{version}")
        archive = self.fetch(f"https://static.crates.io/crates/{name}/{name}-{version}.crate")
        if hashlib.sha256(archive).hexdigest() != record.get("checksum"):
            raise ReleaseError(f"{name}@{version}: archive checksum mismatch")
        # Read a single member in memory; never extract registry-controlled paths.
        member = f"{name}-{version}/.cargo_vcs_info.json"
        try:
            with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
                info_file = tar.extractfile(member)
                if info_file is None:
                    raise ReleaseError(f"{name}@{version}: missing VCS provenance")
                info = json.load(info_file)
        except (KeyError, tarfile.TarError) as error:
            raise ReleaseError(f"{name}@{version}: invalid VCS provenance") from error
        expected_path = Path(package["manifest_path"]).resolve().parent.relative_to(self.root).as_posix()
        vcs = info.get("git", {})
        if vcs.get("sha1") != self.sha or vcs.get("dirty", False) is not False:
            raise ReleaseError(f"{name}@{version}: published source is not clean tag SHA {self.sha}")
        if info.get("path_in_vcs") != expected_path:
            raise ReleaseError(f"{name}@{version}: published source has wrong workspace path")
        print(f"VERIFIED {name}@{version} source={self.sha}", flush=True)
        return True


def publish(packages, registry, run=subprocess.run, sleep=time.sleep):
    failures = []
    for package in packages:
        name = package["name"]
        if registry.verified(package):
            print(f"NO-OP {name}: matching version already uploaded", flush=True)
            continue
        result = run(["cargo", "publish", "--locked", "--registry", "crates-io", "-p", name], check=False)
        # A timed-out upload or an already-uploaded race can be successful.
        # Registry evidence, not Cargo's error wording, decides the outcome.
        for attempt in range(12):
            if registry.verified(package):
                break
            if attempt < 11:
                sleep(10)
        else:
            failures.append(f"{name} (cargo exit {result.returncode})")
    # Recheck all versions after uploads; detect yanks and any still-missing
    # version even if another crate's upload appeared to succeed.
    missing = [p["name"] for p in packages if not registry.verified(p)]
    if missing:
        raise ReleaseError(f"crates.io incomplete: {', '.join(missing)}; upload observations: {failures}")
    print(f"SUCCESS: all {len(packages)} workspace versions verified", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("plan", "publish", "verify"))
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", args.tag):
        raise ReleaseError("Tag must use vMAJOR.MINOR.PATCH")
    sha = command("git", "rev-parse", "HEAD")
    tag_sha = command("git", "rev-parse", "--verify", f"refs/tags/{args.tag}^{{commit}}")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or sha != tag_sha:
        raise ReleaseError("HEAD must be the tag's full commit SHA")
    if command("git", "status", "--porcelain", "--untracked-files=no"):
        raise ReleaseError("Release source has tracked modifications")
    metadata = json.loads(command("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
    packages = publish_order(metadata, args.tag[1:])
    print("ORDER: " + " -> ".join(p["name"] for p in packages), flush=True)
    if args.mode == "plan":
        return
    registry = Registry(sha, metadata["workspace_root"])
    if args.mode == "publish":
        if not os.environ.get("CARGO_REGISTRY_TOKEN"):
            raise ReleaseError("CARGO_REGISTRY_TOKEN missing; workflow must skip publish-crates")
        publish(packages, registry)
    else:
        missing = [p["name"] for p in packages if not registry.verified(p)]
        if missing:
            raise ReleaseError("crates.io incomplete: " + ", ".join(missing))


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, subprocess.CalledProcessError, ValueError, KeyError, OSError) as error:
        print(f"::error::{error}. GitHub Release has not been created or published by this run.", file=sys.stderr)
        sys.exit(1)
