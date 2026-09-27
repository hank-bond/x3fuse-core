# DNG highlight recovery

Highlight recovery estimates brightness and color where sensor layers approach
clipping.

Recovery is off by default. Enable it when writing a Digital Negative (DNG) file:

```sh
x3f_extract -dng -compress -dng-highlight-recovery input.X3F
```

This writes `input.X3F.dng`. Use `-o output` to choose an existing output directory.
The white-balance, denoise, and lens-shading options still apply. Recovery does not
require a particular white-balance preset or a separate calibration file.

## Sensor support

The camera model selects the reconstruction method:

| Camera family | Method |
| --- | --- |
| DP1 Merrill, DP2 Merrill, DP3 Merrill, SD1 Merrill | Estimate brightness, reconstruct its spatial detail, then add color from nearby reliable pixels. |
| Older Foveon cameras | Estimate clipped layers from local layer ratios and the shared color lookup tables, with color safeguards. |
| Quattro | Apply lookup-table and fallback reconstruction after expanding the lower-resolution layers. |

Quattro's expanded layers are not independent full-resolution measurements, so
Quattro does not use the Merrill gradient method. The same output mapping options
are available for all three paths.

## Storing recovered highlights

Recovery can estimate values above the sensor’s recorded clipping limit. For example, a layer records 1.0 because it clipped. Recovery might estimate that its value would have been 1.4 without clipping.  

The mapping controls how these estimates fit into a 16-bit DNG.

### Linear mapping

Linear mapping is the default:

```sh
x3f_extract -dng -compress -dng-highlight-recovery \
  -dng-highlight-mapping linear input.X3F
```

The converter measures the largest recovered value in the active image area. It
uses that value as a shared scale, limited to the range one to 16, and divides the
image by that scale before encoding. Values within this range keep their relative
brightness differences. Values beyond the supported range clip during encoding.

The DNG writer adds the matching exposure adjustment to the `BaselineExposure` tag.
For a scale of two, the stored samples are half as large and the adjustment is one
stop. A reader that ignores this tag displays a darker image. Apply the corresponding
positive exposure adjustment in that reader.

The tags are `BlackLevel = 0`, `WhiteLevel = 65535`, and `LinearResponseLimit = 1`.
Even where recovery leaves a pixel alone, this shared scaling and 16-bit rounding
can make its stored samples differ from a recovery-off DNG. If an uncropped output
includes sensor borders outside the active area, recovery encodes those borders
as black.

### Shoulder mapping

Shoulder mapping compresses the brightest values instead of using an image-wide
exposure scale:

```sh
x3f_extract -dng -compress -dng-highlight-recovery \
  -dng-highlight-mapping shoulder input.X3F
```

The curve starts at `0.85` by default. The `X3F_DNG_SHOULDER_KNEE` environment
variable sets that starting point. Each pixel uses one scale for all three layers,
which preserves their ratios while compressing brightness.

The writer records the knee in `LinearResponseLimit` when compression is needed.
It does not add the linear mapping's headroom adjustment to `BaselineExposure`.
The compression is part of the stored image and cannot be undone by changing a tag.

Both mappings are available, but the Merrill image checks cover linear mapping.
Choosing a mapping with recovery off does not change the recovery-off result.

## Merrill reconstruction

Foveon records light in three overlapping sensor layers. These measurements are
not independent red, green, and blue image channels. Recovery uses a reliability
value for each layer: 255 means fully reliable, zero means clipped, and values in
between describe the approach to clipping.

The following three steps form a unified recovery pipeline, and are not designed to be individually toggled.  The big picture is that this approach follows a sort of "colorization" process. The First estimates the tone (luma) for each single pixel using the three color layers, then we try to optimistically refine the tone based on immediate neighboring values if they are not clipped, then we apply hue (chroma) as estimated from farther neighboring pixels.  

Compared to the other non-Merril approaches, the most importand distinction is that the tone and hue components of the recovered values are measured and estimated discretely.

### 1. Estimate brightness (tone anchor)

The tone "anchor" gives each affected pixel a starting brightness. Where enough
signal remains, it uses brightness calculated from the camera calibration. When
at least two layers have low reliability, it shifts toward an estimate from the
remaining measurements.

Where all layers are clipped, the method can estimate a neutral brightness from
the recorded levels, but those levels provide no measured texture. The estimate
is an assumption about brightness, not a physical measurement of the missing light.

### 2. Reconstruct brightness detail

The tone anchor estimates brightness separately for each affected pixel. This stage
uses spatial changes in the surviving layer measurements to reconstruct brightness
detail that the individual estimates may miss. A *gradient* describes the local
change between neighboring pixels. Here, the change is relative rather than absolute.

For example, two adjacent pixels may have the same recorded value in a clipped
layer, while a reliable layer has a 20% stronger signal at the second pixel. That
ratio supports an estimate that the second pixel is 20% brighter, rather than
equally bright. Other layers and neighbors may support different ratios. A layer
change can also reflect a color difference, so the ratio is evidence for a brightness
change, not proof.

The stage combines this evidence from each pixel's four immediate neighbors at
native resolution. A layer contributes only when both measurements are finite and
positive. The lower of the two reliability values sets the pair's influence.
Zero reliability gives the pair no influence.

The *solver* is the numerical routine that refines brightness estimates across the
affected region together. The anchors provide an overall brightness reference,
while reliable neighboring measurements can move the estimates away from their
starting values. A pixel without usable neighbor evidence keeps its anchor as-is.
Other recovery stages calculate the reliability values and reconstruct highlight
color.

Fully reliable pixels and pixels outside the reconstruction remain fixed during
this calculation. Negative rendered RGB alone does not trigger Merrill recovery.
The solver stops conversion if it cannot produce a valid result. It does not
silently switch to another reconstruction method.

### 3. Add highlight color

Nearby reliable pixels supply color ratios, not brightness or texture. These
pixels are called *donors*. The search uses a fixed 256-pixel radius and reduces
the weight of donors whose ratios disagree with the available layer measurements.
A mix of inconsistent donor colors also lowers confidence.

The color stage scales the selected ratios to the reconstructed brightness.
Without usable donor color, it keeps the tone result. Recovery strength increases
smoothly as layer reliability falls. This blend is applied once, not once per stage.

## Rust API

Use the `ProcessOptions` struct with the `Reader::dump_dng` method:

```rust,no_run
use x3f_core::{DngHighlightMapping, ProcessOptions, Reader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = Reader::open("input.X3F")?;
    let options = ProcessOptions {
        dng_highlight_recovery: true,
        dng_highlight_mapping: DngHighlightMapping::Linear,
        compress: true,
        ..ProcessOptions::default()
    };
    reader.dump_dng("input.X3F.dng", &options)?;
    Ok(())
}
```

The writer selects sensor-space processing. Direct `Reader::get_image` calls need
`ColorEncoding::None` and `cineon: false` to use DNG recovery. TIFF and PPM color
processing do not use the Merrill DNG method. The public C wrapper exposes metadata
and thumbnail access, not DNG conversion.

The library calculates Merrill brightness coefficients from the chosen white
balance and camera profile. It uses the same rounding as the DNG calibration tags.

## Implementation

The following files are under `crates/x3f-sys/src/`:

| File | Responsibility |
| --- | --- |
| `process.rs` | Select the processing path, build shared data, measure headroom, and encode pixels. |
| `highlight_recovery.rs` | Store layer reliability and donor data, and reconstruct older native-sensor highlights. |
| `tone_anchor.rs` | Estimate each affected pixel's starting brightness. |
| `gradient_tone.rs` | Solve for brightness using neighboring layer differences. |
| `donor_chroma.rs` | Estimate nearby color ratios and confidence. |
| `highlight_color.rs` | Apply those ratios at the chosen brightness. |
| `recovery_mask.rs` | Write the optional source-reliability mask. |

The gradient field is built before the headroom and encoding passes. Both passes
read the same field and donor data. Encoding writes pixels in place without reading
neighbors that might already have been overwritten.

Calibration helpers live under `crates/x3f-core/src/output/dng/`. The
`Reader::get_image` method passes their result into processing for each Merrill
conversion. Processing takes that result before starting parallel pixel work.

### Solver iterations

The solver starts with the tone anchors as its brightness estimates, then refines
the affected region:

1. **Compare the estimates with the evidence.** The solver checks how the estimated
   brightness differences agree with the surviving layer measurements, while
   accounting for departures from the anchors.
2. **Calculate a coordinated adjustment.** The solver finds changes to the affected
   pixels that reduce the combined disagreement. Reliable measurements have more
   influence than uncertain ones.
3. **Update and repeat.** The solver applies the adjustment, checks the revised
   estimates, and calculates another adjustment if needed.
4. **Validate the result.** The solver accepts the estimates when they satisfy the
   numerical checks. If the solver cannot reach an acceptable result within its
   processing limit, conversion returns an error.

The measurements, reliability values, and anchors remain unchanged throughout these
iterations. Fully reliable pixels and pixels outside the reconstruction remain
fixed. Only the brightness estimates under reconstruction change.

Neighboring measurements can conflict, so the solver balances their contributions
rather than reproducing every measured ratio exactly. It does not repeatedly
average or blur neighboring pixels. Passing the numerical checks confirms that the
solver completed its calculation, not that the estimates match the original scene.

## Masks, errors, and limits

Use the [recovery mask](./recovery-mask.md) to exclude pixels with stressed source
layers from color-profile training. The mask does not say which output pixels
changed or how much detail is recoverable.

Invalid calibration, missing required layer data, and numerical solver failures
return conversion errors. Solver, headroom, and mask diagnostics use the library's
verbosity and log callback settings. A failed conversion must not be treated as a
usable output.

Recovery estimates brightness and color separately, and either estimate can be
wrong. Differences between neighboring layer measurements may come from a change
in color rather than brightness. Nearby pixels used to estimate highlight color
may belong to a different material.

These errors can make recovered areas too dark or give them the wrong color.
Review the recovered highlights and their transitions into surrounding areas in
the intended DNG reader. A file that opens successfully does not necessarily
contain convincing recovery.
