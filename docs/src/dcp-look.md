# Embedding a DCP look

`-dcp-look FILE` adds a DCP's look table and tone curve to the default embedded
profile in a DNG. The look then travels with the DNG; the editor does not need a
separate profile file. Rendering still depends on the editor honoring these tags.

```sh
x3f_extract -dng -compress -dcp-look look.dcp input.X3F
```

In Rust, set `ProcessOptions::dng_look` to the DCP path and call `Reader::dump_dng`
or `convert_file` with `OutputFormat::Dng`. The option is off by default. It does
not enable highlight recovery or change its mapping. The CLI and `convert_file`
reject this option for non-DNG output. Other low-level extraction methods do not
apply DNG look metadata.

## What is imported

Only these four profile fields are imported:

- `ProfileLookTableDims` (50981)
- `ProfileLookTableData` (50982)
- `ProfileLookTableEncoding` (51108; missing encoding means linear)
- `ProfileToneCurve` (50940)

The curve replaces the default embedded profile's curve rather than stacking
another curve on top. DNGBackwardVersion becomes 1.4 because the look encoding
field requires it. The file is already written as DNG 1.4.

ColorMatrix, ForwardMatrix, camera calibration, white balance, exposure metadata,
raw samples and existing camera hue/saturation maps are not changed by this
option. Other DCP rendering fields, including hue/saturation calibration maps,
exposure offsets and black-render settings, are ignored. This is deliberately
**not full camera-profile application**. The extra selectable camera profiles
remain unchanged; the look belongs to the default embedded profile only.

## Supported package

The reader accepts little- or big-endian extended-profile DCP files up to 16 MiB.
It requires a ProfileName, an exact matching UniqueCameraModel, a complete look
table, and a tone curve. It supports linear (0) and sRGB (1) look encoding.

The reader checks field types, counts, offsets, duplicate tags and finite values.
Table dimensions must agree with the data; saturation/value scales must be
nonnegative and neutral ValueScale must equal one. The tone curve must have
strictly increasing coordinates and run from (0,0) to (1,1). Unsupported or invalid
look data is an error, not a fallback. A profile that forbids embedding is rejected.
These constraints are intentionally narrower than every possible DCP profile.

The public `dcp::DcpLook::{open, from_bytes}` functions can inspect a package before
conversion. They retain only the look data, name and camera restriction, never the
source profile's matrices. Loading a look does not modify an image.

## Preview limitation

The converter embeds metadata; it does not render the look into the raw samples
or its existing small thumbnail. Thumbnail-only viewers can therefore show the
unprofiled appearance. A separate post-processing renderer can refresh the preview
from the completed DNG. A hook doing that must recognize already-matching look
fields rather than apply the look twice.
