#!/usr/bin/env python3
"""Plan and verify Edda's dependency-ordered crates.io release.

This helper is deliberately read-only. It never publishes, yanks, tags, or
changes repository state.
"""

from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


USER_AGENT = "edda-release-skill/1.0 (+https://github.com/fagemx/edda)"


class ReleasePlanError(RuntimeError):
    """A release invariant failed."""


@dataclass(frozen=True)
class Package:
    name: str
    version: str
    dependencies: frozenset[str]


def cargo_metadata(manifest_path: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


def publishable_packages(metadata: dict[str, Any]) -> dict[str, Package]:
    workspace_ids = set(metadata["workspace_members"])
    raw_packages = [
        package
        for package in metadata["packages"]
        if package["id"] in workspace_ids and package.get("publish") != []
    ]
    names = {package["name"] for package in raw_packages}
    if len(names) != len(raw_packages):
        raise ReleasePlanError("publishable workspace crate names are not unique")

    result: dict[str, Package] = {}
    for package in raw_packages:
        internal = frozenset(
            dependency["name"]
            for dependency in package.get("dependencies", [])
            if dependency.get("path") is not None and dependency["name"] in names
        )
        result[package["name"]] = Package(
            name=package["name"],
            version=package["version"],
            dependencies=internal,
        )
    if not result:
        raise ReleasePlanError("no publishable workspace crates found")
    return result


def topological_levels(packages: dict[str, Package]) -> list[list[str]]:
    remaining = {name: set(package.dependencies) for name, package in packages.items()}
    levels: list[list[str]] = []
    published: set[str] = set()

    while remaining:
        ready = sorted(
            name for name, dependencies in remaining.items() if dependencies <= published
        )
        if not ready:
            unresolved = ", ".join(
                f"{name} -> {sorted(dependencies - published)}"
                for name, dependencies in sorted(remaining.items())
            )
            raise ReleasePlanError(f"internal dependency cycle or missing node: {unresolved}")
        levels.append(ready)
        published.update(ready)
        for name in ready:
            del remaining[name]
    return levels


def require_version(packages: dict[str, Package], expected: str) -> None:
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", expected) is None:
        raise ReleasePlanError("--version must be a bare MAJOR.MINOR.PATCH version")
    mismatches = [
        f"{package.name}={package.version}"
        for package in packages.values()
        if package.version != expected
    ]
    if mismatches:
        raise ReleasePlanError(
            f"workspace versions do not all equal {expected}: {', '.join(sorted(mismatches))}"
        )


def select_packages(packages: dict[str, Package], requested: list[str]) -> list[str]:
    if not requested:
        return sorted(packages)
    unknown = sorted(set(requested) - set(packages))
    if unknown:
        raise ReleasePlanError(f"unknown or non-publishable crate(s): {', '.join(unknown)}")
    return sorted(set(requested))


def request_bytes(url: str, timeout: float) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def registry_version(crate: str, version: str, timeout: float) -> dict[str, Any]:
    url = f"https://crates.io/api/v1/crates/{crate}/{version}"
    try:
        payload = json.loads(request_bytes(url, timeout))
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return {"crate": crate, "version": version, "exists": False, "yanked": None}
        raise ReleasePlanError(f"crates.io returned HTTP {error.code} for {crate}") from error
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        raise ReleasePlanError(f"crates.io query failed for {crate}: {error}") from error

    published = payload.get("version")
    if not isinstance(published, dict):
        raise ReleasePlanError(f"crates.io response for {crate} has no version object")
    return {
        "crate": crate,
        "version": version,
        "exists": True,
        "yanked": published.get("yanked"),
        "checksum": published.get("checksum"),
    }


def wait_for_registry(
    crates: list[str],
    version: str,
    wait_seconds: float,
    poll_seconds: float,
    timeout: float,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + wait_seconds
    last_error: ReleasePlanError | None = None
    while True:
        statuses: list[dict[str, Any]] = []
        try:
            statuses = [registry_version(crate, version, timeout) for crate in crates]
            last_error = None
        except ReleasePlanError as error:
            last_error = error

        if last_error is None and all(
            status["exists"] and status["yanked"] is False for status in statuses
        ):
            return statuses
        if time.monotonic() >= deadline:
            if last_error is not None:
                raise last_error
            return statuses
        time.sleep(min(poll_seconds, max(0.0, deadline - time.monotonic())))


def vcs_sha_from_archive(archive: bytes, label: str) -> str:
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as crate:
            members = [
                member
                for member in crate.getmembers()
                if member.name.endswith("/.cargo_vcs_info.json")
            ]
            if len(members) != 1:
                raise ReleasePlanError(
                    f"{label} contains {len(members)} .cargo_vcs_info.json files"
                )
            extracted = crate.extractfile(members[0])
            if extracted is None:
                raise ReleasePlanError(f"cannot read provenance from {label}")
            payload = json.load(extracted)
    except (tarfile.TarError, json.JSONDecodeError) as error:
        raise ReleasePlanError(f"cannot parse package provenance from {label}: {error}") from error

    git = payload.get("git", {})
    sha = git.get("sha1")
    if not isinstance(sha, str):
        raise ReleasePlanError(f"{label} has no git.sha1 provenance")
    if git.get("dirty", False) is not False:
        raise ReleasePlanError(f"{label} was packaged from a dirty worktree")
    return sha.lower()


def validate_expected_sha(value: str) -> str:
    normalized = value.lower()
    if len(normalized) != 40 or any(character not in "0123456789abcdef" for character in normalized):
        raise ReleasePlanError("--expected-sha must be a full 40-hex commit SHA")
    return normalized


def local_package_provenance(
    package_dir: Path, crates: list[str], version: str, expected_sha: str
) -> dict[str, str]:
    result: dict[str, str] = {}
    for crate in crates:
        archive_path = package_dir / f"{crate}-{version}.crate"
        if not archive_path.is_file():
            raise ReleasePlanError(f"missing packaged archive: {archive_path}")
        sha = vcs_sha_from_archive(archive_path.read_bytes(), str(archive_path))
        if sha != expected_sha:
            raise ReleasePlanError(
                f"{archive_path.name} provenance {sha} does not match {expected_sha}"
            )
        result[crate] = sha
    return result


def published_package_provenance(
    crates: list[str], version: str, expected_sha: str, timeout: float
) -> dict[str, str]:
    result: dict[str, str] = {}
    for crate in crates:
        url = f"https://crates.io/api/v1/crates/{crate}/{version}/download"
        try:
            archive = request_bytes(url, timeout)
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as error:
            raise ReleasePlanError(f"cannot download {crate} {version}: {error}") from error
        sha = vcs_sha_from_archive(archive, f"{crate} {version} from crates.io")
        if sha != expected_sha:
            raise ReleasePlanError(
                f"published {crate} provenance {sha} does not match {expected_sha}"
            )
        result[crate] = sha
    return result


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--manifest-path", type=Path, default=Path("Cargo.toml"))
    result.add_argument("--version", required=True, help="bare MAJOR.MINOR.PATCH version")
    result.add_argument("--crate", action="append", default=[], dest="crates")
    result.add_argument("--commands", action="store_true", help="print cargo publish commands")
    result.add_argument("--json", action="store_true", dest="json_output")
    result.add_argument("--check-crates-io", action="store_true")
    result.add_argument(
        "--allow-missing",
        action="store_true",
        help="inventory missing/yanked versions instead of failing (recovery only)",
    )
    result.add_argument(
        "--wait-crates-io",
        type=float,
        metavar="SECONDS",
        help="poll until selected crates exist and are unyanked",
    )
    result.add_argument("--poll-interval", type=float, default=5.0)
    result.add_argument("--network-timeout", type=float, default=20.0)
    result.add_argument("--package-dir", type=Path)
    result.add_argument("--expected-sha")
    result.add_argument("--check-published-provenance", action="store_true")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        metadata = cargo_metadata(args.manifest_path.resolve())
        packages = publishable_packages(metadata)
        require_version(packages, args.version)
        levels = topological_levels(packages)
        selected = select_packages(packages, args.crates)

        if args.poll_interval <= 0:
            raise ReleasePlanError("--poll-interval must be positive")
        if args.network_timeout <= 0:
            raise ReleasePlanError("--network-timeout must be positive")
        if args.package_dir is not None and args.expected_sha is None:
            raise ReleasePlanError("--package-dir requires --expected-sha")
        if args.check_published_provenance and args.expected_sha is None:
            raise ReleasePlanError("--check-published-provenance requires --expected-sha")
        if args.allow_missing and args.check_published_provenance:
            raise ReleasePlanError(
                "--allow-missing cannot be combined with --check-published-provenance"
            )
        expected_sha = (
            validate_expected_sha(args.expected_sha) if args.expected_sha is not None else None
        )

        registry: list[dict[str, Any]] | None = None
        if args.wait_crates_io is not None:
            if args.wait_crates_io < 0:
                raise ReleasePlanError("--wait-crates-io must be non-negative")
            registry = wait_for_registry(
                selected,
                args.version,
                args.wait_crates_io,
                args.poll_interval,
                args.network_timeout,
            )
        elif args.check_crates_io or args.check_published_provenance:
            registry = wait_for_registry(
                selected, args.version, 0, args.poll_interval, args.network_timeout
            )

        if registry is not None:
            unavailable = [
                status["crate"]
                for status in registry
                if not status["exists"] or status["yanked"] is not False
            ]
            if unavailable and not args.allow_missing:
                raise ReleasePlanError(
                    f"crates.io exact version missing or yanked: {', '.join(unavailable)}"
                )

        local_provenance: dict[str, str] | None = None
        if args.package_dir is not None and expected_sha is not None:
            local_provenance = local_package_provenance(
                args.package_dir.resolve(), selected, args.version, expected_sha
            )

        published_provenance: dict[str, str] | None = None
        if args.check_published_provenance and expected_sha is not None:
            published_provenance = published_package_provenance(
                selected, args.version, expected_sha, args.network_timeout
            )

        output = {
            "version": args.version,
            "publishable_count": len(packages),
            "selected_count": len(selected),
            "selected": selected,
            "levels": levels,
            "registry": registry,
            "local_provenance": local_provenance,
            "published_provenance": published_provenance,
        }

        if args.json_output:
            print(json.dumps(output, indent=2, sort_keys=True))
        else:
            print(f"publishable workspace crates: {len(packages)}")
            for index, level in enumerate(levels):
                print(f"L{index}: {', '.join(level)}")
            if args.commands:
                print("publish commands:")
                for level in levels:
                    for crate in level:
                        if crate in selected:
                            print(f"cargo publish -p {crate} --locked")
            if registry is not None:
                unavailable = [
                    status["crate"]
                    for status in registry
                    if not status["exists"] or status["yanked"] is not False
                ]
                if unavailable:
                    print(
                        "crates.io inventory: "
                        f"{len(registry) - len(unavailable)} ready, "
                        f"{len(unavailable)} missing/yanked"
                    )
                else:
                    print(f"crates.io exact-version parity: PASS ({len(registry)} checked)")
            if local_provenance is not None:
                print(f"local package provenance: PASS ({len(local_provenance)} checked)")
            if published_provenance is not None:
                print(f"published package provenance: PASS ({len(published_provenance)} checked)")
        return 0
    except (ReleasePlanError, subprocess.CalledProcessError) as error:
        print(f"release preflight failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
