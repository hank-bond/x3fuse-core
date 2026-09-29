# Frozen Merrill N2 regression outputs

`recovery_n2.json` pins 32 complete DNG/mask outputs from the accepted N2
production converter, before cleanup. It covers 11 DP2 Merrill inputs:
0004, 0009, 0010, 0014, 0031, 0032, 0095, 0168, 0170, 0195 and 0373.
The private X3F inputs and full golden files are not redistributed here.
Input hashes distinguish versions even when filenames are reused.

Coverage includes default linear recovery, mask enabled/disabled, shoulder,
recovery disabled under both mapping selections, disabled denoise or bad-pixel
repair, uncompressed DNG, and Overcast white balance. Defaults include denoise
strength 10. The baseline was generated on macOS arm64; it is not evidence of
other-camera or cross-platform parity.

From the workspace root:

```sh
cargo build --release
python3 scripts/check_recovery_regression.py \
  --binary target/release/x3f_extract \
  --fixtures /path/to/frozen-x3f-inputs \
  --golden /path/to/frozen-output-case-directories \
  --output /path/to/new-check-directory
```

Python 3.9+ and its standard library suffice. `--golden` is optional: the manifest
always checks complete-file SHA-256 and byte counts; golden files add direct
byte-for-byte comparisons. DNG comparison includes pixels, metadata and previews.
There are no pixel tolerances, timestamp exemptions, or automatic baseline updates.
The checker clears inherited recovery-research environment variables.

Missing/changed inputs, changed goldens, output differences, unexpected files,
and conversion failures **fail**, rather than silently skipping. The output
directory must be new. Each successful run records its binary/manifest hashes
and results in `result.json`. Keep failed output for diagnosis; do not re-pin a
mismatch merely to accept a refactor.

The checker itself has small fixture-independent failure tests:

```sh
python3 scripts/test_check_recovery_regression.py
```

This exact gate is specific to the N2 cleanup, alongside existing unit and corpus
tests. It does not change upstream's general testing policy or prove photographic
quality. Compare actual C1 renders with fixed settings for that separate review.
