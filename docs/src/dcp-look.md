# Embedding a DCP look

Use `-dcp-look FILE` to add a DCP look table and tone curve to a DNG:

```sh
x3f_extract -dng -compress -dcp-look look.dcp input.X3F
```

In Rust, set `ProcessOptions::dng_look` and use `Reader::dump_dng` or
`convert_file` with `OutputFormat::Dng`. The CLI and `convert_file` reject the
option for other formats. The lower-level TIFF and PPM writers do not apply it.

The option is off by default and does not enable highlight recovery. Without a
look, output is unchanged.

## What changes

The look belongs to the default embedded profile. Its tone curve replaces the
existing curve. Import copies only these fields:

- `ProfileLookTableDims`
- `ProfileLookTableData`
- `ProfileLookTableEncoding`
- `ProfileToneCurve`

Camera calibration, white balance, exposure metadata, raw samples, and the other
embedded profiles stay unchanged. The DNG backward version becomes 1.4 to support
the look encoding tag. This is look import, not replacement of the camera profile.

## Accepted profiles

The reader accepts little-endian and big-endian extended-profile DCP files up to
16 MiB. A file must have a profile name, an exact matching `UniqueCameraModel`,
a complete look table, and a tone curve. The curve must increase strictly from
`(0,0)` to `(1,1)`. Table values must be finite, scales must be nonnegative, and
neutral value scales must equal one. Supported encodings are linear and sRGB.
An omitted encoding means linear.

Malformed files, unsupported look values, and profiles that forbid embedding
produce an error. The converter does not substitute another look. Rust callers
can inspect a file with `dcp::DcpLook::open` or parse bytes with `from_bytes`.

## Previews

The option changes metadata, not the small embedded thumbnail. A viewer must
render the DNG using its embedded profile to show the look. A separate preview
renderer can use the completed DNG without applying the DCP again.
