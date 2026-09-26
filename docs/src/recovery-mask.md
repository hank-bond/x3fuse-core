# Native recovery exclusion sidecar

For masked LUT preparation, request a binary PGM sidecar with the optional
`-dng-recovery-mask` switch:

```sh
x3f_extract -dng-highlight-recovery -dng-recovery-mask -wb Daylight -denoise 10 -compress -o output input.X3F
```

The output directory must already exist. This writes `input.X3F.dng` and
`input.X3F.mask.pgm` into `output`. Without `-o`, both are beside the input. Batch
conversion derives a separate name for each input; duplicate destinations are
rejected. The mask name is independent of the DNG's temporary `.dng.tmp` name.
Daylight here is the training reference recipe, not a restriction on supported WB.

Mask export is **off by default** and requires DNG output with highlight recovery
enabled. It does not enable recovery implicitly or change its mapping. There is
no embedded-mask option. The former `X3F_DIAGNOSTIC_RECOVERY_MASK` environment
interface has been removed; active consumers must migrate explicitly. Archived
controllers and their paired binaries should not be repointed to a new binary.

## Library callers

Set `ProcessOptions::dng_recovery_mask` to an optional destination path:

```rust,ignore
let options = ProcessOptions {
    dng_highlight_recovery: true,
    dng_recovery_mask: Some("input.X3F.mask.pgm".into()),
    ..ProcessOptions::default()
};
reader.dump_dng("input.X3F.dng", &options)?;
```

The DNG writer selects the native processing path as usual. Direct `get_image`
callers must also select `ColorEncoding::None` and disable Cineon. Requests and
results are isolated per conversion, including nested Rayon work. Each simultaneous
conversion must have its own output paths. The mask path must differ from the DNG
path; it is not a template or a directory.

## Meaning and geometry

The binary PGM (`P5`, maximum 255) covers the **native active area**, before EXIF
rotation:

- `0`: all three source-layer reliability codes are 255 (fully healthy).
- `255`: at least one layer is below 255 (partial stress or clipping).

Evidence includes native camera clipping maps merged by the existing recovery
model. This is a conservative source-stress mask, not a count of pixels recovery
changed, a recovery ON/OFF difference image, or an estimate of recoverable detail.
Ineligible/unrecoverable stressed pixels are deliberately excluded too.

The exporter reads the immutable model before in-place encoding and changes no
recovery values, parameters, headroom or metadata. It applies no dilation or
rotation. Training must transform the mask alongside image geometry and choose its
boundary margin explicitly (the current workflow uses an 8-pixel square dilation).

## Errors and partial outputs

Creation is exclusive: existing masks, including symlinks, are never overwritten.
Invalid requests, missing native reliability, or create/write/flush/sync failures
return ordinary conversion errors, not panics or silent omissions. Native evidence
availability is checked; it is not inferred merely from a camera name.

The sidecar is written during processing, before DNG publication. The DNG retains
its existing CLI temporary-file/rename lifecycle. These two files are **not an
atomic transaction**: a partial mask, a complete mask without a DNG, or a DNG
temporary file can remain after failure. There is no automatic rollback or deletion
of sidecars. Inspect failed outputs before retrying with a fresh destination.

Sidecar existence alone does not prove success. Check the conversion result,
validate the DNG and mask, and bind source/output/mask hashes before training.
