#!/usr/bin/env python3
"""Build controller/agent compatibility manifests from prebuilt Linux binaries."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import tomllib

PLATFORMS = {"linux-amd64": (2, 62), "linux-arm64": (2, 183), "linux-armv7": (1, 40)}
ROLES = ("hproxy-agent", "sing-box", "realm", "hproxy-server")


def require_static_linux(path):
    """One Linux catalog must run on both glibc hosts and musl containers."""
    with path.open("rb") as stream:
        header = stream.read(64)
        if len(header) < 52 or header[:4] != b"\x7fELF" or header[5] != 1:
            raise ValueError(f"Invalid Linux ELF header: {path}")
        if header[4] == 2 and len(header) >= 64:
            offset = struct.unpack_from("<Q", header, 32)[0]
            entry, count = struct.unpack_from("<HH", header, 54)
            minimum = 56
        elif header[4] == 1:
            offset = struct.unpack_from("<I", header, 28)[0]
            entry, count = struct.unpack_from("<HH", header, 42)
            minimum = 32
        else:
            raise ValueError(f"Invalid Linux ELF class: {path}")
        if not count or count > 1024 or entry < minimum or offset < minimum or offset + entry * count > path.stat().st_size:
            raise ValueError(f"Invalid Linux program headers: {path}")
        for i in range(count):
            stream.seek(offset + entry * i)
            if struct.unpack("<I", stream.read(4))[0] == 3:  # PT_INTERP
                raise ValueError(f"Linux releases require static binaries; use the musl build: {path}")


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True, help="12 prebuilt binaries named <role>-<platform>")
    parser.add_argument("--output", type=Path, required=True, help="new, empty release directory")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    common = (root / "crates/common/src/lib.rs").read_text(encoding="utf-8")
    sing = re.search(r'SING_VERSION: &str = "([^"]+)"', common)[1]
    realm = re.search(r'REALM_VERSION: &str = "([^"]+)"', common)[1]
    versions = dict(zip(ROLES, (version, sing, realm, version)))
    # Validate the entire input before creating any publishable output.
    manifests = {}
    for platform, (elf_class, machine) in PLATFORMS.items():
        artifacts = {}
        for role in ROLES:
            file = f"{role}-{platform}"
            path = args.input / file
            size = path.stat().st_size
            with path.open("rb") as stream:
                header = stream.read(20)
            if not (20 <= size <= 150 * 1024 * 1024 and header[:4] == b"\x7fELF" and header[4] == elf_class and header[5] == 1 and int.from_bytes(header[18:20], "little") == machine):
                raise SystemExit(f"Invalid Linux architecture/size: {path}")
            try:
                require_static_linux(path)
            except ValueError as error:
                raise SystemExit(str(error)) from None
            artifacts[role] = {"file": file, "sha256": sha256(path), "bytes": size, "version": versions[role]}
        sums = "".join(f"{artifacts[role]['sha256']}  {artifacts[role]['file']}\n" for role in ROLES[:3])
        build = hashlib.sha256(sums.encode()).hexdigest()
        manifests[platform] = ({"schema": 1, "build": build, "version": version, "agent": artifacts[ROLES[0]], "sing_box": artifacts[ROLES[1]], "realm": artifacts[ROLES[2]], "server": artifacts[ROLES[3]]}, sums)
    args.output.mkdir(parents=True, exist_ok=False)
    for platform, (manifest, sums) in manifests.items():
        (args.output / f"manifest-{platform}.json").write_text(json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
        (args.output / f"SHA256SUMS-{platform}").write_text(sums, encoding="ascii", newline="\n")
        server = manifest["server"]
        (args.output / f"SHA256SUMS-server-{platform}").write_text(f"{server['sha256']}  {server['file']}\n", encoding="ascii", newline="\n")
        for role in ROLES:
            file = f"{role}-{platform}"
            shutil.copyfile(args.input / file, args.output / file)
    shutil.copyfile(root / "scripts/install-server.sh", args.output / "install-server.sh")
    shutil.copyfile(root / "scripts/install-agent.sh", args.output / "install-agent.sh")
    shutil.copyfile(root / "scripts/uninstall-server.sh", args.output / "uninstall-server.sh")
    shutil.copyfile(root / "scripts/agentctl.sh", args.output / "agentctl.sh")
    shutil.copyfile(root / "scripts/uninstall-agent.sh", args.output / "uninstall-agent.sh")
    print(f"Compatibility release {version}: {args.output}")


if __name__ == "__main__":
    main()
