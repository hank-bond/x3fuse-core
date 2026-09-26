//! Dedicated cloud-transition fixture from0358, upright (1280,735)/(1280,736).
//! Captured native proposals are inputs, not asserted physical ground truth.
//! Both source masks are [255,255,0]. No image corpus or environment needed.

use super::*;
use crate::highlight_recovery::{protect_highlight_color, stabilize_highlight_color};

const MATRIX: [f64; 9] = [
    4.569666635259227,
    -1.9150458294139914,
    0.5670006700527498,
    -4.334345135931211,
    4.317628468323228,
    -0.6834763376981136,
    3.2163803048225605,
    -5.834933429568362,
    3.231780599487513,
];
const PRIOR: [f64; 3] = [0.27013614426683996, 0.5865495678883843, 1.0];
const BLACK: [f64; 3] = [51.12927515750404; 3];
const WHITE: [u32; 3] = [16383, 7573, 4463];
const PIXELS: [[u16; 3]; 2] = [[5097, 5523, 4479], [5089, 5530, 4479]];
const GAINS: [[f64; 3]; 2] = [
    [1.018099106075503, 0.9833092165394114, 1.0129691096856221],
    [1.0180262760206842, 0.9832593726853404, 1.0128787979482976],
];
const LOCAL: [[f64; 3]; 2] = [
    [0.314550400311489, 0.7153195145563316, 1.3043282556010856],
    [0.31402922907562336, 0.7161982954772881, 1.2942332544035193],
];

fn captured_lut() -> chroma_lut_t {
    let mut lut: chroma_lut_t = unsafe { std::mem::zeroed() };
    lut.valid = 1;
    lut.sat_threshold = 0.99;
    lut.soft_window = 0.2;
    lut.asymmetric_max = 0.95;
    lut.recovery_cap = 1.75;
    lut.blend_threshold = 0.75;
    lut.blend_divisor = 0.1;
    lut.neutral_tm = 1.704885750065529;
    // Exact f32 entries from the frozen image-wide LUT. Neither is a
    // missing bin; bin75's +3.84% deviation is rejected by the5% veto.
    lut.lut[74] = 1.7963425;
    lut.lut[75] = 1.7703627;
    lut.lut[76] = 1.4925402;
    lut.lut[77] = 1.4266403;
    lut
}

fn native_reference(
    original: [f64; 3],
    gain: [f64; 3],
    local: [f64; 3],
    confidence: f64,
    mask: [u8; 3],
    lut: &chroma_lut_t,
) -> [f64; 3] {
    use crate::highlight_recovery::{LocalRecovery, RecoveryResult, SensorReliability};
    let mut reliability = SensorReliability::new(1, 1, [0.001; 3]).unwrap();
    reliability.data[0] = mask;
    let model = LocalRecovery::build(reliability, |_, _| original, None).unwrap();
    let prior = std::array::from_fn(|c| PRIOR[c] / gain[c]);
    let proposal = RecoveryResult {
        samples: std::array::from_fn(|c| local[c] / gain[c]),
        confidence,
        recovered: confidence > 0.0,
        damaged: true,
    };
    let reference = model
        .color_reference(0, 0, original, prior, Some(lut), proposal)
        .unwrap();
    let stable = std::array::from_fn(|c| reference.samples[c] * gain[c]);
    let measured = std::array::from_fn(|c| original[c] * gain[c]);
    let stable = protect_highlight_color(stable, measured, mask, PRIOR, &MATRIX, 0.2, 0.3);
    stabilize_highlight_color(local, stable, measured, mask, &MATRIX)
}

fn reference_replay(index: usize) -> [f64; 3] {
    let original = std::array::from_fn(|c| {
        (PIXELS[index][c] as f64 - BLACK[c]) / (WHITE[c] as f64 - BLACK[c])
    });
    native_reference(
        original,
        GAINS[index],
        LOCAL[index],
        [0.4752779108585011, 0.6651927195351368][index],
        [255, 255, 0],
        &captured_lut(),
    )
}

fn rgb_share(samples: [f64; 3]) -> [f64; 3] {
    let rgb = mat3x1_mul_native(&MATRIX, samples);
    let sum = rgb.iter().sum::<f64>();
    assert!(sum > 0.0);
    rgb.map(|v| v / sum)
}

fn max_delta(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f64::max)
}

#[test]
fn cloud_735_736_continuity_regression() {
    // Case-specific guardrail, not a global/perceptual acceptance score.
    // Neutralizing both pixels could pass this, so color preservation and
    // real-image review are separately required for any proposed fix.
    let step = max_delta(
        rgb_share(reference_replay(0)),
        rgb_share(reference_replay(1)),
    );
    assert!(step < 0.02, "adjacent cloud chromaticity jump: {step}");
    for (i, local) in LOCAL.into_iter().enumerate() {
        let actual = rgb_share(reference_replay(i));
        assert!(
            actual[2] > actual[0].max(actual[1]),
            "must retain supported blue, not merely neutralize both"
        );
        assert!(max_delta(actual, rgb_share(local)) < 0.005);
    }
}

#[test]
fn cloud_native_reference_has_no_lookup_boundary_jump() {
    let middle = 0.7284;
    let boundary = 76.0 * middle / (255.0 - 76.0);
    let evaluate = |b| {
        native_reference(
            [b, middle, 1.0036265795164638],
            GAINS[0],
            LOCAL[0],
            0.4752779108585011,
            [255, 255, 0],
            &captured_lut(),
        )
    };
    let step = max_delta(
        rgb_share(evaluate(boundary - 1e-9)),
        rgb_share(evaluate(boundary + 1e-9)),
    );
    assert!(step < 1e-7, "reference boundary step: {step}");
}

#[test]
fn cloud_native_reference_requires_supported_local_color() {
    let original = [0.3089585271555692, 0.7274614155186873, 1.0036265795164638];
    let result = native_reference(
        original,
        GAINS[0],
        LOCAL[0],
        0.0,
        [255, 255, 0],
        &captured_lut(),
    );
    let rgb = rgb_share(result);
    assert!(max_delta(rgb, [1.0 / 3.0; 3]) < 1e-6);
    let mut coherent_neutral = captured_lut();
    coherent_neutral
        .lut
        .fill(coherent_neutral.neutral_tm as f32);
    let result = native_reference(
        original,
        GAINS[0],
        LOCAL[0],
        1.0,
        [255, 255, 0],
        &coherent_neutral,
    );
    assert!(max_delta(rgb_share(result), [1.0 / 3.0; 3]) < 1e-6);
}
