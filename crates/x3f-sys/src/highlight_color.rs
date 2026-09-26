//! Highlight color reconstruction on an established scalar-tone target.
//! No tone filtering, reference CLUT, gamut snap or extra onset ramp.

use super::tone_anchor;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

pub(super) fn apply(
    measured: [f64; 3],
    tone: &tone_anchor::Result,
    neutral: [f64; 3],
    y: [f64; 3],
    direction: [f64; 3],
    confidence: f64,
) -> [f64; 3] {
    if !tone.available
        || tone.strength <= 0.0
        || !confidence.is_finite()
        || confidence <= 0.0
        || direction.iter().any(|v| !v.is_finite() || *v <= 0.0)
    {
        return tone.samples;
    }
    let target = neutral.map(|v| v * tone.amplitude);
    let target_y = dot(y, target);
    let donor_y = dot(y, direction);
    if !target_y.is_finite() || target_y <= 0.0 || !donor_y.is_finite() || donor_y <= 1e-12 {
        return tone.samples;
    }
    let colored = direction.map(|v| v * (target_y / donor_y));
    if colored.iter().any(|v| !v.is_finite()) {
        return tone.samples;
    }
    let weight = confidence.clamp(0.0, 1.0);
    // Color the target, then use the ORIGINAL single tone-onset blend. Mixing
    // an already-onset-blended image with donor color would apply onset twice.
    std::array::from_fn(|c| {
        let target = target[c] + weight * (colored[c] - target[c]);
        if tone.strength == 1.0 {
            target
        } else {
            measured[c] + tone.strength * (target - measured[c])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const P: [f64; 3] = [0.27, 0.59, 1.0];
    const Y: [f64; 3] = [-2.79537849, 3.41456424, -0.2476];
    const D: [f64; 3] = [0.28, 0.58, 1.17];
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} != {b}");
    }

    #[test]
    fn color_preserves_any_scalar_tone_through_the_entire_onset() {
        let measured = [0.32, 0.72, 1.0];
        for reliability in 0..=255 {
            let t = tone_anchor::recover(measured, [255, 255, reliability], P);
            for confidence in [0.0, 0.001, 0.25, 0.5, 1.0] {
                let c = apply(measured, &t, P, Y, D, confidence);
                close(dot(Y, c), dot(Y, t.samples));
            }
        }
    }
    #[test]
    fn healthy_and_no_survivor_samples_are_exactly_untouched() {
        for s in [[0.32, 0.72, 1.0], [-0.01, 0.4, 0.9], [0.5, 0.1, 0.9]] {
            for mask in [[255; 3], [0; 3]] {
                let t = tone_anchor::recover(s, mask, P);
                assert_eq!(apply(s, &t, P, Y, D, 1.0), s);
            }
        }
    }
    #[test]
    fn missing_or_invalid_color_support_returns_exact_tone() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 0], P);
        for d in [[0.0; 3], [f64::NAN, 1.0, 1.0], [1.0, 0.01, 1.0]] {
            let c = apply(s, &t, P, Y, d, 1.0);
            assert_eq!(c, t.samples);
        }
        for confidence in [0.0, -1.0, f64::NAN] {
            assert_eq!(apply(s, &t, P, Y, D, confidence), t.samples);
        }
    }
    #[test]
    fn full_confidence_keeps_donor_color_at_tone_brightness() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 0], P);
        let c = apply(s, &t, P, Y, D, 1.0);
        for i in 1..3 {
            close(c[i] / D[i], c[0] / D[0]);
        }
        assert!((c[2] / c[1] - P[2] / P[1]).abs() > 0.1);
    }
    #[test]
    fn chroma_fades_linearly_without_moving_luminance() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 0], P);
        let full = apply(s, &t, P, Y, D, 1.0);
        for confidence in [0.01, 0.25, 0.75] {
            let c = apply(s, &t, P, Y, D, confidence);
            for i in 0..3 {
                close(c[i] - t.samples[i], confidence * (full[i] - t.samples[i]));
            }
        }
    }
    #[test]
    fn donor_intensity_cannot_change_color_or_tone() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 0], P);
        let reference = apply(s, &t, P, Y, D, 0.75);
        for scale in [0.1, 2.0, 10.0] {
            let c = apply(s, &t, P, Y, D.map(|v| v * scale), 0.75);
            for i in 0..3 {
                close(c[i], reference[i]);
            }
        }
    }
    #[test]
    fn partial_onset_is_not_applied_twice() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 207], P);
        let scale = dot(Y, P.map(|v| v * t.amplitude)) / dot(Y, D);
        let c = apply(s, &t, P, Y, D, 1.0);
        for i in 0..3 {
            close(c[i], s[i] + t.strength * (scale * D[i] - s[i]));
        }
    }
    #[test]
    fn existing_neutral_chromaticity_does_not_add_a_cast() {
        let s = [0.32, 0.72, 1.0];
        let t = tone_anchor::recover(s, [255, 255, 180], P);
        let c = apply(s, &t, P, Y, P, 0.8);
        for i in 0..3 {
            close(c[i], t.samples[i]);
        }
    }
}
