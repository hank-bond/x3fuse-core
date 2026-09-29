"""Small synthetic tests for the strict gate, independent of private X3F fixtures."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name("check_recovery_regression.py")


def pin(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


class RegressionGateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fixtures = self.root / "fixtures"
        self.fixtures.mkdir()
        (self.fixtures / "input.X3F").write_bytes(b"input")
        self.golden = self.root / "golden"
        (self.golden / "case").mkdir(parents=True)
        self.payloads = {"input.X3F.dng": b"DNG pixels", "input.X3F.mask.pgm": b"mask"}
        for name, data in self.payloads.items():
            (self.golden / "case" / name).write_bytes(data)
        self.manifest = self.root / "manifest.json"
        self.manifest.write_text(json.dumps({"schema": 1, "cases": [{
            "id": "case", "input": "input.X3F", "input_sha256": pin(b"input")["sha256"],
            "args": [], "outputs": {name: pin(data) for name, data in self.payloads.items()},
        }]}))
        self.manifest_before = self.manifest.read_bytes()
        self.binary = self.root / "converter"
        self.output = self.root / "output"

    def run_gate(self, payloads=None):
        # A deterministic stand-in lets these tests corrupt exactly one output byte.
        self.binary.write_text(
            f"#!{sys.executable}\nimport sys\nfrom pathlib import Path\n"
            "output=Path(sys.argv[sys.argv.index('-o')+1])\n"
            f"for name,data in {(self.payloads if payloads is None else payloads)!r}.items():\n"
            " (output/name).write_bytes(data)\n")
        self.binary.chmod(0o700)
        result = subprocess.run([
            sys.executable, str(RUNNER), "--binary", str(self.binary),
            "--fixtures", str(self.fixtures), "--golden", str(self.golden),
            "--manifest", str(self.manifest), "--output", str(self.output),
        ], capture_output=True, text=True)
        self.assertEqual(self.manifest.read_bytes(), self.manifest_before)
        return result

    def test_exact_files_pass_and_record_direct_byte_check(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.output / "result.json").read_text())
        self.assertTrue(report["direct_byte_comparison"])
        self.assertTrue(report["cases"][0]["whole_files_exact"])

    def test_one_changed_dng_byte_fails_without_repinning(self):
        result = self.run_gate(self.payloads | {"input.X3F.dng": b"DNG pixelX"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("differs from frozen N2", result.stderr)

    def test_changed_mask_fails(self):
        result = self.run_gate(self.payloads | {"input.X3F.mask.pgm": b"Mask"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("mask.pgm differs", result.stderr)

    def test_changed_input_fails_before_conversion(self):
        (self.fixtures / "input.X3F").write_bytes(b"other")
        self.assertNotEqual(self.run_gate().returncode, 0)
        self.assertFalse(self.output.exists())

    def test_missing_input_is_not_skipped(self):
        (self.fixtures / "input.X3F").unlink()
        self.assertNotEqual(self.run_gate().returncode, 0)
        self.assertFalse(self.output.exists())

    def test_existing_output_is_not_overwritten(self):
        self.output.mkdir()
        (self.output / "keep").write_text("untouched")
        self.assertNotEqual(self.run_gate().returncode, 0)
        self.assertEqual((self.output / "keep").read_text(), "untouched")

    def test_unexpected_output_fails(self):
        result = self.run_gate(self.payloads | {"unexpected.pgm": b"extra"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected/missing outputs", result.stderr)

    def test_changed_golden_is_not_silently_accepted(self):
        (self.golden / "case/input.X3F.dng").write_bytes(b"DNG pixelX")
        self.assertNotEqual(self.run_gate().returncode, 0)


if __name__ == "__main__":
    unittest.main()
