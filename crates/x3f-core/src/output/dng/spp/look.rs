//! Bake the pointwise color residual around Adobe's exposure and tone operations.
//!
//! The table uses hue, saturation, and sRGB-encoded value coordinates. It changes
//! rendering metadata only. Black and neutral entries remain identity transforms,
//! while the separate profile curve carries neutral brightness.

use super::{
    color_dq::ColorDq,
    invalid, mul, srgb_decode, srgb_encode,
    tone::{inverse_rgb_tone, tone_at, SATURATION},
};
use crate::Error;
use x3f_sys::Control;

// Coarse hue, saturation, and value sampling. These dimensions control table
// size and interpolation error, not the strength of the native correction.
pub(super) const DIMS: [u32; 3] = [72, 33, 65];

// DNG encoding 1 spaces value samples in encoded sRGB rather than linear RGB.
// The dense shadow sampling matters for the small ColorDQ correction.
pub(super) const ENCODING_SRGB: u32 = 1;

// A nearly neutral RGB triplet has no stable hue. Keep its hue shift zero.
// These tolerances avoid divisions by tiny channel spans and saturation values.
const NEUTRAL_SPAN_EPSILON: f64 = 1e-12;
const HUE_SATURATION_EPSILON: f64 = 1e-7;
const DEGREES_PER_TURN: f64 = 360.0;

// HSV has six hue sectors. Offsets select the red, green, and blue triangle
// waves. The falling triangle edge starts at sector four. The two non-red
// hue branches start at sectors two and four.
const HUE_SECTORS: f64 = 6.0;
const RGB_HUE_OFFSETS: [f64; 3] = [5.0, 3.0, 1.0];
const TRIANGLE_FALLING_OFFSET: f64 = 4.0;
const GREEN_HUE_SECTOR: f64 = 2.0;
const BLUE_HUE_SECTOR: f64 = 4.0;

/// Bake a table using the camera adapter after Adobe's positive-exposure ramp.
/// Supply the combined exposure and profile response as the effective tone.
pub(super) fn bake(
    adapter: &[f64; 9],
    output: &[f64; 9],
    tone: &[f64],
    effective_tone: &[f64],
    dq: &ColorDq,
    control: Control<'_>,
) -> Result<Vec<f32>, Error> {
    let [nh, ns, nv] = DIMS;
    let mut table = Vec::with_capacity((nh * ns * nv * 3) as usize);
    for v in 0..nv {
        control.check()?;
        let encoded_value = v as f64 / (nv - 1) as f64;
        for h in 0..nh {
            let hue = h as f64 / nh as f64;
            for s in 0..ns {
                let sat = s as f64 / (ns - 1) as f64;
                if v == 0 || s == 0 {
                    table.extend([0.0, 1.0, 1.0]);
                    continue;
                }
                let rgb = hsv_to_rgb(hue, sat, srgb_decode(encoded_value));
                let counts = dq.apply(mul(adapter, rgb).map(|x| x * SATURATION as f64));
                let target = mul(
                    output,
                    counts.map(|x| tone_at(tone, x / SATURATION as f64) / tone[SATURATION]),
                )
                .map(|x| x.clamp(0.0, 1.0));
                let target = inverse_rgb_tone(target, effective_tone);
                let [target_hue, target_saturation, target_value] = rgb_to_hsv(target);
                let shift = if target_saturation < HUE_SATURATION_EPSILON {
                    0.0
                } else {
                    ((target_hue - hue + 0.5).rem_euclid(1.0) - 0.5) * DEGREES_PER_TURN
                };
                let saturation = target_saturation / sat;
                let value = srgb_encode(target_value) / encoded_value;
                if !shift.is_finite() || !saturation.is_finite() || !value.is_finite() {
                    return Err(invalid("invalid generated look table"));
                }
                table.extend([shift as f32, saturation as f32, value as f32]);
            }
        }
    }
    Ok(table)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    RGB_HUE_OFFSETS.map(|offset| {
        let k = (h * HUE_SECTORS + offset) % HUE_SECTORS;
        v * (1.0 - s * k.min(TRIANGLE_FALLING_OFFSET - k).clamp(0.0, 1.0))
    })
}

fn rgb_to_hsv(rgb: [f64; 3]) -> [f64; 3] {
    let hi = rgb.into_iter().fold(0.0, f64::max);
    let lo = rgb.into_iter().fold(f64::INFINITY, f64::min);
    let d = hi - lo;
    if d <= NEUTRAL_SPAN_EPSILON || hi <= 0.0 {
        return [0.0, 0.0, hi];
    }
    let hue = if rgb[0] == hi {
        (rgb[1] - rgb[2]) / d
    } else if rgb[1] == hi {
        (rgb[2] - rgb[0]) / d + GREEN_HUE_SECTOR
    } else {
        (rgb[0] - rgb[1]) / d + BLUE_HUE_SECTOR
    };
    [(hue / HUE_SECTORS).rem_euclid(1.0), d / hi, hi]
}

#[cfg(test)]
mod tests {
    use super::super::tone::{native_tone, ToneShape};
    use super::*;

    const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    #[test]
    fn hsv_coordinates_round_trip() {
        for h in 0..72 {
            for s in [0.1, 0.5, 1.0] {
                let hsv = [h as f64 / 72.0, s, 0.6];
                let actual = rgb_to_hsv(hsv_to_rgb(hsv[0], s, hsv[2]));
                for i in 0..3 {
                    assert!((actual[i] - hsv[i]).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn color_residual_obeys_adobe_constraints() {
        let tone = native_tone(ToneShape {
            start: -1.17_f32 as f64,
            end: 1.65_f32 as f64,
            lower_steepness: 3.0,
            breakpoint: 0.1_f32 as f64,
            upper_steepness: 1.7_f32 as f64,
        });
        let dq = ColorDq::new([8.0; 3]).unwrap();
        let table = bake(&IDENTITY, &IDENTITY, &tone, &tone, &dq, Control::none()).unwrap();
        assert!(table
            .as_chunks::<3>()
            .0
            .iter()
            .all(|entry| entry.iter().all(|v| v.is_finite())
                && entry[1] >= 0.0
                && entry[2] >= 0.0));
        for entry in table.as_chunks::<3>().0.iter().step_by(DIMS[1] as usize) {
            assert_eq!(entry, &[0.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn generation_obeys_cancellation() {
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        assert!(matches!(
            bake(
                &IDENTITY,
                &IDENTITY,
                &vec![0.0; 4096],
                &vec![0.0; 4096],
                &ColorDq::new([8.0; 3]).unwrap(),
                Control::new(&cancelled)
            ),
            Err(Error::Cancelled)
        ));
    }
}
