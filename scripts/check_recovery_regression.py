#!/usr/bin/env python3
"""Check frozen N2 whole-DNG/mask outputs. Never creates or updates golden hashes.

Private X3F fixtures are not redistributed. Missing or changed inputs are errors,
not skipped tests. See crates/x3f-cli/tests/data/README.md for usage and limits.
"""
import argparse
import concurrent.futures
import filecmp
import hashlib
import json
import os
from pathlib import Path
import subprocess


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def check_case(case, binary, fixtures, output, golden, environment):
    work = output / case["id"]
    work.mkdir()
    with (work / "convert.log").open("x") as log:
        subprocess.run(
            [str(binary), "-o", str(work), *case["args"], str(fixtures / case["input"])],
            env=environment, stdout=log, stderr=subprocess.STDOUT, check=True,
        )
    expected = case["outputs"]
    actual = {p.name for p in work.iterdir() if p.name != "convert.log"}
    require(actual == set(expected), f"{case['id']}: unexpected/missing outputs: {actual}")
    for name, pinned in expected.items():
        path = work / name
        require(path.stat().st_size == pinned["bytes"] and sha256(path) == pinned["sha256"],
                f"{case['id']}: {name} differs from frozen N2; do not re-pin")
        if golden:
            reference = golden / case["id"] / name
            require(sha256(reference) == pinned["sha256"], f"Golden changed: {reference}")
            require(filecmp.cmp(path, reference, shallow=False), f"Byte mismatch: {path}")
    print(f"EXACT {case['id']}", flush=True)
    return {"id": case["id"], "whole_files_exact": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--fixtures", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="New directory; no overwrites")
    parser.add_argument("--golden", type=Path, help="Optional frozen files for direct byte comparisons")
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parents[1]
                        / "crates/x3f-cli/tests/data/recovery_n2.json")
    parser.add_argument("--jobs", type=int, choices=(1, 2), default=2)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    require(manifest["schema"] == 1 and manifest["cases"], "Invalid/empty manifest")
    cases = manifest["cases"]
    require(len({c["id"] for c in cases}) == len(cases), "Duplicate case IDs")
    inputs = {}
    for case in cases:
        previous = inputs.setdefault(case["input"], case["input_sha256"])
        require(previous == case["input_sha256"], "Conflicting input pins")
    for name, expected in inputs.items():
        require(sha256(args.fixtures / name) == expected, f"Input changed: {name}")
    args.output.mkdir(parents=True, exist_ok=False)
    environment = {k: v for k, v in os.environ.items() if not k.startswith(
        ("X3F_", "A_TRACE_", "COLOR_FIELD_", "CONNECTED_COLOR_", "SKY_"))}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(lambda case: check_case(
            case, args.binary.resolve(), args.fixtures.resolve(), args.output,
            args.golden, environment), cases))
    report = {"binary_sha256": sha256(args.binary), "manifest_sha256": sha256(args.manifest),
              "direct_byte_comparison": args.golden is not None, "cases": results}
    (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"All {len(results)} frozen N2 cases match exactly.")


if __name__ == "__main__":
    main()
