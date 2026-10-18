import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release_package", Path(__file__).resolve().parents[1] / "scripts/package-release.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class StaticReleaseTests(unittest.TestCase):
    def elf(self, bits, interpreter=False, machine=0):
        header = bytearray(64 if bits == 64 else 52)
        header[:6] = b"\x7fELF" + bytes([2 if bits == 64 else 1, 1])
        struct.pack_into("<H", header, 18, machine)
        entry = 56 if bits == 64 else 32
        struct.pack_into("<Q" if bits == 64 else "<I", header, 32 if bits == 64 else 28, len(header))
        struct.pack_into("<HH", header, 54 if bits == 64 else 42, entry, 1)
        segment = bytearray(entry)
        struct.pack_into("<I", segment, 0, 3 if interpreter else 1)
        return header + segment

    def test_static_32_and_64_bit_programs_are_accepted(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "core"
            for bits in [32, 64]:
                path.write_bytes(self.elf(bits))
                package.require_static_linux(path)

    def test_dynamic_loader_and_truncated_program_headers_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "core"
            for bits in [32, 64]:
                path.write_bytes(self.elf(bits, True))
                with self.assertRaisesRegex(ValueError, "musl"):
                    package.require_static_linux(path)
                path.write_bytes(self.elf(bits)[:-1])
                with self.assertRaisesRegex(ValueError, "program headers"):
                    package.require_static_linux(path)

    def test_package_contains_only_amd64_controller_agent_and_cores(self):
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "binaries"
            bundle = Path(folder) / "bundle"
            source.mkdir()
            self.assertEqual(list(package.PLATFORMS), ["linux-amd64"])
            for platform, (elf_class, machine) in package.PLATFORMS.items():
                for role in package.ROLES:
                    (source / f"{role}-{platform}").write_bytes(
                        self.elf(64 if elf_class == 2 else 32, machine=machine)
                    )
            with patch.object(sys, "argv", ["package-release.py", "--input", str(source), "--output", str(bundle)]):
                package.main()
            manifest = json.loads((bundle / "manifest-linux-amd64.json").read_text())
            for role, key in zip(package.ROLES, ("agent", "sing_box", "realm", "server")):
                self.assertEqual(manifest[key]["file"], f"{role}-linux-amd64")
                self.assertTrue((bundle / f"{role}-linux-amd64").is_file())
            self.assertEqual(list(bundle.glob("manifest-*.json")), [bundle / "manifest-linux-amd64.json"])


if __name__ == "__main__":
    unittest.main()
