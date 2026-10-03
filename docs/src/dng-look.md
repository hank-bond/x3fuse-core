# Embedding a DNG look

Use `-dng-look FILE` to add a DCP look table and tone curve to a DNG:

```sh
x3f_extract -dng -compress -dng-look look.dcp input.X3F
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

The selected look applies to every DNG conversion, regardless of camera model.
The DCP's `UniqueCameraModel` is neither required nor used to restrict import.
A look's appearance on other cameras still needs visual review.

## Accepted profiles

The reader accepts little-endian and big-endian extended-profile DCP files up to
16 MiB. A file must have a profile name, a complete look table, and a tone curve.
The curve must increase strictly from
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

## Experimental CAMF Standard rendering

Use `-dng-spp-standard -wb Daylight` to generate rendering metadata from a DP2 Merrill X3F's camera metadata (CAMF). The converter requires no external DCP file or Sigma Photo Pro (SPP) runtime. The option is disabled by default. This experimental recipe uses a ×2 setup gain, Adobe RGB internal color routing, native-derived Standard tone and metadata-derived ColorDQ. It does not reproduce the complete SPP pipeline.

```sh
x3f_extract -dng -wb Daylight -denoise 10 -dng-highlight-recovery -dng-highlight-mapping linear -dng-spp-standard input.X3F
```

The writer embeds a coarse `[72,33,65]` look table and a separate 513-point tone curve. Baking uses the DNG's published calibration and rounded exposure. The tone curve carries neutral brightness, including compensation for Adobe's white-preserving negative-exposure adjustment. Its highlight tail can be flat. The look table carries the color residual around Adobe's hue-preserving tone operator. ColorDQ follows the camera matrix and setup gain, before tone. The setup gain does not multiply ColorDQ's correction amplitude. No `BaselineExposureOffset` is written.

Camera samples, calibration, white balance, `BaselineExposure`, headroom storage and generated previews remain identical to conversion with the same processing options and this flag disabled. N2, metadata-based bad-pixel repair and the existing denoiser remain separate upstream operations. This option adds no sharpening, Fill Light, lens correction or native SPP detail reconstruction. The small embedded preview does not show the new look. Render the DNG with its default embedded profile to review it.

The option requires DNG output, linear highlight mapping and Daylight/Sunlight on a DP2 Merrill. Do not combine it with `-dng-look`. Unsupported calibration or tone parameters return an error. The recipe is reviewed with Adobe RGB source metadata, not other internal color routes. Adobe clips some values before profile finishing, and the coarse table interpolates the native correction. Exact native equivalence is not guaranteed. Other white-balance edits and readers need separate review.
