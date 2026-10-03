//! Generate the Standard tone response and its Adobe profile representation.
//!
//! Native tone applies independently to each channel. Adobe instead maps the
//! lowest and highest channels, then interpolates the middle channel. Inverse
//! tone evaluation lets the look table compensate for that difference.

use super::srgb_decode;

// The verified Merrill branch builds its varying tone response below code 4095.
// Higher inputs repeat the final value. This is a rendering limit, not a change
// to the DNG's linear camera samples or recovered headroom.
pub(super) const SATURATION: usize = 4095;

// Native tone warps the input before its two sigmoid segments and samples the
// resulting curve every 50 codes to form Bezier controls. Output quantization
// uses the full unsigned 16-bit range.
const INPUT_WARP: f64 = 2.2;
const CONTROL_STEP: usize = 50;
const OUTPUT_MAX: f64 = 65535.0;

// A 513-point profile curve carries neutral brightness. The look table uses a
// finer 4096-point evaluation of the combined exposure and profile response.
const PROFILE_INTERVALS: usize = 512;

// Adobe joins its negative-exposure shadow line to a quadratic at one quarter
// of the input range. The quadratic preserves slope at the join and maps white
// to white. Its curvature coefficient is 1 / (1 - 0.25)^2.
const EXPOSURE_JOIN: f64 = 0.25;
const EXPOSURE_CURVATURE: f64 = 16.0 / 9.0;
const EXPOSURE_SLOPE_OFFSET: f64 = 0.5;

// Bisection bounds inverse-exposure error below 2^-48 on the unit interval.
// The channel-span tolerance avoids division by a nearly neutral RGB range.
const INVERSE_EXPOSURE_STEPS: usize = 48;
const NEUTRAL_SPAN_EPSILON: f64 = 1e-12;

/// Generate the normal Standard branch from five CAMF tone-shape parameters.
pub(super) fn native_tone(p: [f64; 5]) -> Vec<f64> {
    let [start, end, steep1, breakpoint, steep2] = p;
    let join = (breakpoint * SATURATION as f64).ceil() as usize;
    let warped =
        |i: usize| start + (end - start) * (i as f64 / SATURATION as f64).powf(1.0 / INPUT_WARP);
    let sigmoid = |steep: f64, i: usize| 1.0 / (1.0 + (-steep * warped(i)).exp());
    let delta = sigmoid(steep1, join) - sigmoid(steep2, join);
    let raw = |i: usize| {
        if i < join {
            sigmoid(steep1, i) as f32
        } else {
            (sigmoid(steep2, i) + delta) as f32
        }
    };
    let mut controls: Vec<f32> = (0..=SATURATION).step_by(CONTROL_STEP).map(raw).collect();
    controls.push(raw(SATURATION));
    let low = bezier(&controls, 0.0);
    let high = bezier(&controls, 1.0);
    (0..=SATURATION)
        .map(|i| {
            let encoded = (bezier(&controls, i.min(SATURATION - 1) as f32 / SATURATION as f32)
                - low)
                / (high - low);
            (srgb_decode(encoded) * OUTPUT_MAX + 0.5).floor() / OUTPUT_MAX
        })
        .collect()
}

/// Carry neutral brightness without changing neutral look-table value scales.
pub(super) fn profile_curve(tone: &[f64], neutral_scale: f64, exposure: f64) -> Vec<f32> {
    (0..=PROFILE_INTERVALS)
        .flat_map(|i| {
            let x = i as f64 / PROFILE_INTERVALS as f64;
            let y = if i == PROFILE_INTERVALS {
                1.0
            } else {
                tone_at(tone, neutral_scale * inverse_exposure_tone(x, exposure)) / tone[SATURATION]
            };
            [x as f32, y as f32]
        })
        .collect()
}

pub(super) fn tone_at(table: &[f64], value: f64) -> f64 {
    let p = (value * SATURATION as f64).clamp(0.0, SATURATION as f64);
    let i = p.floor() as usize;
    table[i] + (table[(i + 1).min(SATURATION)] - table[i]) * (p - i as f64)
}

pub(super) fn curve_at(curve: &[f32], x: f64) -> f64 {
    let p = x.clamp(0.0, 1.0) * PROFILE_INTERVALS as f64;
    let i = p.floor() as usize;
    let lo = curve[i * 2 + 1] as f64;
    let hi = curve[i.min(PROFILE_INTERVALS - 1) * 2 + 3] as f64;
    lo + (hi - lo) * (p - i as f64)
}

pub(super) fn exposure_tone(x: f64, exposure: f64) -> f64 {
    if exposure >= 0.0 {
        return x;
    }
    let slope = 2.0_f64.powf(exposure);
    if x <= EXPOSURE_JOIN {
        return x * slope;
    }
    let a = EXPOSURE_CURVATURE * (1.0 - slope);
    let b = slope - EXPOSURE_SLOPE_OFFSET * a;
    let c = 1.0 - a - b;
    (a * x + b) * x + c
}

pub(super) fn inverse_rgb_tone(rgb: [f64; 3], tone: &[f64]) -> [f64; 3] {
    let low = rgb.into_iter().fold(f64::INFINITY, f64::min);
    let high = rgb.into_iter().fold(0.0, f64::max);
    let a = inverse_tone(tone, low);
    let b = inverse_tone(tone, high);
    if high - low <= NEUTRAL_SPAN_EPSILON {
        return [a; 3];
    }
    rgb.map(|v| a + (b - a) * (v - low) / (high - low))
}

fn bezier(controls: &[f32], t: f32) -> f64 {
    let mut work = controls.to_vec();
    for n in (1..work.len()).rev() {
        for i in 0..n {
            work[i] = t.mul_add(work[i + 1], (1.0 - t) * work[i]);
        }
    }
    work[0] as f64
}

fn inverse_exposure_tone(y: f64, exposure: f64) -> f64 {
    if exposure >= 0.0 || y <= 0.0 || y >= 1.0 {
        return y;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..INVERSE_EXPOSURE_STEPS {
        let mid = (lo + hi) * 0.5;
        if exposure_tone(mid, exposure) < y {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5
}

fn inverse_tone(tone: &[f64], y: f64) -> f64 {
    if y <= 0.0 {
        return 0.0;
    }
    if y >= tone[SATURATION] {
        return 1.0;
    }
    let right = tone.partition_point(|v| *v < y).clamp(1, SATURATION);
    let left = right - 1;
    (left as f64 + (y - tone[left]) / (tone[right] - tone[left])) / SATURATION as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD: [f64; 5] = [
        -1.17_f32 as f64,
        1.65_f32 as f64,
        3.0,
        0.1_f32 as f64,
        1.7_f32 as f64,
    ];

    #[test]
    fn normal_tone_matches_saved_native_standard_samples() {
        let tone = native_tone(STANDARD);
        for (x, y) in [
            (0.001, 0.00041371),
            (0.01, 0.00383759),
            (0.1, 0.11451653),
            (0.18, 0.25236463),
            (0.5, 0.69801989),
            (0.75, 0.89309104),
        ] {
            assert!(
                (tone_at(&tone, x) - y).abs() < 2.0 / 65535.0,
                "{x}: {} != {y}",
                tone_at(&tone, x)
            );
        }
        assert_eq!(tone[0], 0.0);
        assert_eq!(tone[4094], tone[4095]);
        assert!(tone.windows(2).all(|p| p[1] >= p[0]));
    }

    #[test]
    fn neutral_curve_carries_gain_without_a_profile_exposure_offset() {
        let tone = native_tone(STANDARD);
        for exposure in [-1.0_f64, -0.4, 0.0, 2.0] {
            let scale = 1.4 * 2.0_f64.powf(exposure.min(0.0));
            let curve = profile_curve(&tone, scale, exposure);
            assert_eq!(&curve[..2], &[0.0, 0.0]);
            assert_eq!(&curve[curve.len() - 2..], &[1.0, 1.0]);
            // The native shoulder reaches a constant tail. Adobe permits this.
            assert!(curve
                .windows(4)
                .step_by(2)
                .all(|p| p[2] > p[0] && p[3] >= p[1]));
            for x in [0.01, 0.05, 0.18, 0.4, 0.7] {
                let actual = curve_at(&curve, exposure_tone(x, exposure));
                let expected = tone_at(&tone, x * scale) / tone[SATURATION];
                assert!(
                    (actual - expected).abs() < 0.0002,
                    "{exposure} {x}: {actual} {expected}"
                );
                assert!(
                    (inverse_exposure_tone(exposure_tone(x, exposure), exposure) - x).abs() < 1e-12
                );
            }
        }
    }

    #[test]
    fn inverse_rgb_tone_preserves_hue_operator_coordinates() {
        let tone = native_tone(STANDARD);
        let curve = profile_curve(&tone, 1.0, 0.0);
        assert_eq!(&curve[..2], &[0.0, 0.0]);
        assert_eq!(&curve[curve.len() - 2..], &[1.0, 1.0]);
        assert!(curve
            .windows(4)
            .step_by(2)
            .all(|p| p[2] > p[0] && p[3] > p[1]));
        for exposure in [1.5, -0.4] {
            let effective: Vec<f64> = (0..=SATURATION)
                .map(|i| {
                    curve_at(
                        &curve,
                        exposure_tone(i as f64 / SATURATION as f64, exposure),
                    )
                })
                .collect();
            for target in [[0.05, 0.2, 0.7], [0.3, 0.3, 0.3], [0.0, 0.8, 1.0]] {
                let pre = inverse_rgb_tone(target, &effective);
                let low = pre.into_iter().fold(f64::INFINITY, f64::min);
                let high = pre.into_iter().fold(0.0, f64::max);
                let a = tone_at(&effective, low);
                let b = tone_at(&effective, high);
                let actual = if high - low < 1e-12 {
                    [a; 3]
                } else {
                    pre.map(|x| a + (b - a) * (x - low) / (high - low))
                };
                for i in 0..3 {
                    assert!((actual[i] - target[i]).abs() < 1e-9);
                }
            }
        }
        assert_eq!(exposure_tone(0.1, -1.0), 0.05);
        assert!((exposure_tone(1.0, -1.0) - 1.0).abs() < 1e-12);
    }
}
