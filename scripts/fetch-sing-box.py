#!/usr/bin/env python3
"""Fetch the pinned upstream static musl sing-box asset for one Linux target."""
import argparse
import hashlib
from pathlib import Path
import re
import tarfile
import tempfile
import urllib.request

VERSION = "1.14.3"
ARCHIVES = {
    "linux-amd64": ("amd64", "41fc81fd041134d4d7d850a928d7d2cde17a7d6563953f406b22d32a68bfab1f"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=ARCHIVES, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    common = (root / "crates/common/src/lib.rs").read_text(encoding="utf-8")
    configured = re.search(r'SING_VERSION: &str = "([^"]+)"', common)
    if not configured or configured.group(1) != VERSION:
        raise SystemExit("Update this pinned archive table together with SING_VERSION")
    arch, expected = ARCHIVES[args.platform]
    name = f"sing-box-{VERSION}-linux-{arch}-musl.tar.gz"
    url = f"https://github.com/SagerNet/sing-box/releases/download/v{VERSION}/{name}"
    args.output.mkdir(parents=True, exist_ok=True)
    destination = args.output / f"sing-box-{args.platform}"
    if destination.exists():
        raise SystemExit(f"Refusing to overwrite {destination}")
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / name
        digest = hashlib.sha256()
        size = 0
        with urllib.request.urlopen(url, timeout=60) as response, archive.open("wb") as output:
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > 100 * 1024 * 1024:
                    raise SystemExit("Upstream archive exceeded size limit")
                digest.update(chunk)
                output.write(chunk)
        if digest.hexdigest() != expected:
            raise SystemExit(f"Upstream archive SHA-256 mismatch: {name}")
        with tarfile.open(archive, "r:gz") as bundle:
            matches = [item for item in bundle if item.isfile() and Path(item.name).name == "sing-box"]
            if len(matches) != 1 or matches[0].size > 150 * 1024 * 1024:
                raise SystemExit(f"Unexpected archive layout: {name}")
            source = bundle.extractfile(matches[0])
            if source is None:
                raise SystemExit(f"Unable to read binary: {name}")
            with source, destination.open("xb") as output:
                while chunk := source.read(1024 * 1024):
                    output.write(chunk)
    destination.chmod(0o755)
    print(f"{destination}: {destination.stat().st_size} bytes; archive sha256 {expected}")


if __name__ == "__main__":
    main()
