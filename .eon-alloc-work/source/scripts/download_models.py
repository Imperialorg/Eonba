#!/usr/bin/env python3
"""EON model downloader with fail-closed integrity checks and hardware profiles.

Pins live in scripts/models.lock.json. An entry without a verified sha256 is refused, so nothing is
fetched on trust. Profiles (p0 to p3) expand into model ids through named groups. Downloads resume from
<dest>.part via HTTP Range, are verified by streaming SHA-256, fsynced, and atomically moved into place.

Usage:
    python scripts/download_models.py --profile p1_mobile_flagship --dry-run
    python scripts/download_models.py --profile p1_mobile_flagship
    python scripts/download_models.py --ids zipformer-en-2023-06-26-tokens
"""
import argparse
import hashlib
import json
import os
import sys
import tarfile
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LOCK_PATH = Path(__file__).resolve().parent / "models.lock.json"
SUPPORTED_SCHEMA = 3
CHUNK = 1 << 20


class LockError(Exception):
    pass


def load_lock(path=LOCK_PATH):
    lock = json.loads(path.read_text(encoding="utf-8"))
    if lock.get("schema_version") != SUPPORTED_SCHEMA:
        raise LockError(f"unsupported lock schema {lock.get('schema_version')} in {path}")
    return lock


def resolve_profile(lock, profile):
    """Expands a profile into ordered, de-duplicated model ids. Raises LockError on unknown names."""
    profiles = lock["profiles"]
    if profile not in profiles:
        raise LockError(f"unknown profile '{profile}'; choose from {', '.join(sorted(profiles))}")
    groups = lock.get("groups", {})
    known = {m["id"] for m in lock["models"]}
    ordered = []
    for name in profiles[profile]:
        members = groups.get(name, [name])
        for model_id in members:
            if model_id not in known:
                raise LockError(f"profile '{profile}' references unknown model '{model_id}'")
            if model_id not in ordered:
                ordered.append(model_id)
    return ordered


def select_entries(lock, ids=None, profile=None):
    if profile:
        wanted = resolve_profile(lock, profile)
    elif ids:
        wanted = ids
    else:
        wanted = None
    by_id = {m["id"]: m for m in lock["models"]}
    if wanted is None:
        return list(lock["models"])
    missing = [i for i in wanted if i not in by_id]
    if missing:
        raise LockError(f"unknown model ids: {', '.join(missing)}")
    return [by_id[i] for i in wanted]


def is_pinned(entry):
    digest = entry.get("sha256")
    return isinstance(digest, str) and len(digest) == 64 and entry.get("url") is not None


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch_resumable(url, part):
    existing = part.stat().st_size if part.exists() else 0
    request = urllib.request.Request(url, headers={"User-Agent": "eon-model-fetch"})
    if existing:
        request.add_header("Range", f"bytes={existing}-")
    try:
        response = urllib.request.urlopen(request)
    except urllib.error.HTTPError as err:
        if err.code == 416:  # the .part file already holds the whole object
            return
        raise
    with response:
        resumed = existing > 0 and response.status == 206
        with open(part, "ab" if resumed else "wb") as out:
            for chunk in iter(lambda: response.read(CHUNK), b""):
                out.write(chunk)
            out.flush()
            os.fsync(out.fileno())


def ensure_archive(archive, root):
    """Fetches a pinned archive into the gitignored download cache once and verifies its digest."""
    cache = root / "perception" / "models" / "downloads" / Path(archive["url"]).name
    if cache.exists() and sha256_file(cache) == archive["sha256"]:
        return cache
    cache.parent.mkdir(parents=True, exist_ok=True)
    part = cache.with_name(cache.name + ".part")
    print(f"[DOWNLOADING] archive {cache.name}")
    fetch_resumable(archive["url"], part)
    actual = sha256_file(part)
    if actual != archive["sha256"]:
        part.unlink()
        raise RuntimeError(f"archive digest mismatch for {cache.name}: expected {archive['sha256']}, got {actual}")
    os.replace(part, cache)
    return cache


def extract_member(archive_path, member_name, dest_part):
    """Streams one regular file out of a tar archive. Only the named member is read; paths are never used."""
    with tarfile.open(archive_path, "r:bz2") as tar:
        matches = [m for m in tar.getmembers() if m.isfile() and Path(m.name).name == member_name]
        if len(matches) != 1:
            raise RuntimeError(f"expected exactly one '{member_name}' in {archive_path.name}, found {len(matches)}")
        source = tar.extractfile(matches[0])
        with open(dest_part, "wb") as out:
            for chunk in iter(lambda: source.read(CHUNK), b""):
                out.write(chunk)
            out.flush()
            os.fsync(out.fileno())


def commit(part, dest):
    os.replace(part, dest)
    dir_fd = os.open(dest.parent, os.O_RDONLY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def install(entry, root):
    dest = root / entry["dest"]
    expected = entry["sha256"]
    if dest.exists() and sha256_file(dest) == expected:
        print(f"[VERIFIED] {entry['id']} already present at {dest}")
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    if "archive" in entry:
        archive = entry["archive"]
        print(f"[EXTRACTING] {entry['id']} from pinned archive -> {dest}")
        archive_path = ensure_archive(archive, root)
        extract_member(archive_path, archive["member"], part)
        size = part.stat().st_size
        if entry.get("size_bytes") is not None and size != entry["size_bytes"]:
            part.unlink()
            raise RuntimeError(f"size mismatch for {entry['id']}: expected {entry['size_bytes']}, got {size}")
    else:
        print(f"[DOWNLOADING] {entry['id']} -> {dest}")
        fetch_resumable(entry["url"], part)
    actual = sha256_file(part)
    if actual != expected:
        part.unlink()
        raise RuntimeError(f"digest mismatch for {entry['id']}: expected {expected}, got {actual}")
    commit(part, dest)
    print(f"[COMPLETED] {entry['id']} verified and committed")


def print_plan(entries):
    unpinned = []
    for entry in entries:
        status = "PINNED" if is_pinned(entry) else "UNPINNED"
        size = entry.get("size_bytes")
        size_text = f"{size / 1e6:,.1f} MB" if isinstance(size, int) else "size unknown"
        print(f"{status:9} {entry['id']:36} {size_text:>14}  {entry['dest']}")
        if not is_pinned(entry):
            unpinned.append(entry["id"])
    return unpinned


def main(argv=None, lock_path=LOCK_PATH, root=REPO_ROOT):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--profile", help="hardware profile: p0_mobile_ultralight, p1_mobile_flagship, p2_workstation_16g, p3_workstation_heavy")
    parser.add_argument("--ids", nargs="*", help="only fetch these model ids")
    parser.add_argument("--dry-run", action="store_true", help="print the plan and exit without network access")
    parser.add_argument("--root", type=Path, default=root, help="directory that receives the dest paths")
    args = parser.parse_args(argv)

    try:
        entries = select_entries(load_lock(lock_path), ids=args.ids, profile=args.profile)
    except LockError as err:
        print(f"[ERROR] {err}", file=sys.stderr)
        return 2

    if args.dry_run:
        unpinned = print_plan(entries)
        return 1 if unpinned else 0

    failures = 0
    for entry in entries:
        if not is_pinned(entry):
            print(f"[UNPINNED] {entry['id']} has no verified sha256; refusing to download", file=sys.stderr)
            failures += 1
            continue
        try:
            install(entry, args.root)
        except Exception as err:  # keep going so one bad artifact does not hide the others
            print(f"[ERROR] {entry['id']}: {err}", file=sys.stderr)
            failures += 1
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
