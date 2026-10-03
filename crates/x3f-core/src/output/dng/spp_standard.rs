//! Embed a CAMF-derived Standard look without changing linear camera samples.
//!
//! The camera adapter maps Adobe's rendering coordinates into the native tone
//! and ColorDQ input range. A separate profile curve carries neutral brightness.
//! Spatial processing stays upstream. This recipe does not reproduce the full
//! Sigma Photo Pro pipeline.

mod color_dq;
mod look;
mod tone;

use x3f_sys::{self as sys, Control};

use super::{
    color::ColorCalibration,
    metadata::{mat3_diag, mat3_inverse, mat3_mul},
    profiles, tags,
    tiff_writer::{DirectoryWriter, Value},
};
use crate::{Error, Reader};
use color_dq::ColorDq;
use tone::{curve_at, exposure_tone, native_tone, profile_curve, SATURATION};

// Rendering setup

// The supported native setup branch adds one stop before ColorDQ and tone.
// This gain belongs in rendering metadata, not in the stored camera samples.
const SETUP_GAIN: f64 = 2.0;

// CAMF stores eight color-mode rows with seven values per row. Standard uses
// the first row. Keep the stored dimensions when reading the complete matrix.
const TONE_MODE_COUNT: usize = 8;
const TONE_PARAMETERS_PER_MODE: usize = 7;

// Match the writer's rounding for ForwardMatrix and BaselineExposure. Baking
// against unrounded calibration produces a different look in the Adobe SDK.
const TAG_DENOMINATOR: i32 = 10000;

// Adobe's profile connection space uses this D50 white and the ROMM primaries.
// Normalize matrix rows to that white before deriving the camera adapter.
const PCS_WHITE: [f64; 3] = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
const ROMM: [f64; 9] = [
    0.7977, 0.1352, 0.0313, 0.2880, 0.7119, 0.0001, 0.0, 0.0, 0.8249,
];

// Native Adobe RGB to ROMM conversion at zero saturation adjustment. Native
// source 3 selects Adobe RGB, and destination 11 selects ROMM. These are the
// native 12-bit fixed-point coefficients, not a replacement camera calibration.
// Preserve the slight row-sum difference caused by native quantization.
const MATRIX_SCALE: f64 = 4096.0;
const ADOBE_TO_ROMM: [f64; 9] = [
    3032.0 / MATRIX_SCALE,
    464.0 / MATRIX_SCALE,
    601.0 / MATRIX_SCALE,
    564.0 / MATRIX_SCALE,
    3412.0 / MATRIX_SCALE,
    120.0 / MATRIX_SCALE,
    97.0 / MATRIX_SCALE,
    302.0 / MATRIX_SCALE,
    3697.0 / MATRIX_SCALE,
];

// Standard sRGB transfer constants serve two different boundaries: native
// tone decoding and DNG look-table value encoding. They do not reinterpret
// the camera samples as sRGB.
const SRGB_ENCODED_JOIN: f64 = 0.04045;
const SRGB_LINEAR_JOIN: f64 = 0.0031308;
const SRGB_SLOPE: f64 = 12.92;
const SRGB_OFFSET: f64 = 0.055;
const SRGB_SCALE: f64 = 1.055;
const SRGB_POWER: f64 = 2.4;

pub(super) struct SppStandard {
    camera: [f64; 9],
    gain: [f64; 3],
    black: [f64; 3],
    iso: f64,
    tone: Vec<f64>,
    dq: ColorDq,
}

impl SppStandard {
    /// Capture calibration before processing changes the decoded sensor raster.
    /// Require DP2 Merrill data and the Daylight or Sunlight white-balance preset.
    pub(super) fn prepare(reader: &Reader, wb: &str, control: Control<'_>) -> Result<Self, Error> {
        control.check()?;
        if reader.dng_prop("CAMMODEL").as_deref() != Some("SIGMA DP2 Merrill")
            || !matches!(wb, "Daylight" | "Sunlight")
        {
            return Err(invalid("only DP2 Merrill Daylight/Sunlight is verified"));
        }
        let camera = reader
            .dng_camf_matrix_3x3("SunlightCCMatrix")
            .ok_or_else(|| invalid("missing Sunlight matrix"))?;
        let gain = reader
            .dng_gain(Some(wb))
            .ok_or_else(|| invalid("missing gain"))?;
        let capture = reader
            .dng_camf_float("CaptureISO")
            .ok_or_else(|| invalid("missing capture ISO"))?;
        let sensor = reader
            .dng_camf_float("SensorISO")
            .ok_or_else(|| invalid("missing sensor ISO"))?;
        let iso = capture / sensor;
        let mut area: sys::x3f_area16_t = unsafe { std::mem::zeroed() };
        let mut black = [0.0; 3];
        let mut deviation = [0.0; 3];
        let mut maximum = [0; 3];
        let mut settings = [0.0; TONE_MODE_COUNT * TONE_PARAMETERS_PER_MODE];
        let mut amplitudes = [0.0; 3];
        // SAFETY: all reads use the caller's loaded reader, with bounded output arrays.
        let valid = unsafe {
            sys::x3f_image_area(reader.x3f.as_ptr(), &mut area) != 0
                && sys::get_black_level(
                    reader.x3f.as_ptr(),
                    &mut area,
                    1, // Match preprocessing's shield-rectangle scaling.
                    3, // Merrill has three measured layers.
                    black.as_mut_ptr(),
                    deviation.as_mut_ptr(),
                ) != 0
                && sys::x3f_get_max_raw(reader.x3f.as_ptr(), maximum.as_mut_ptr()) != 0
                && sys::x3f_get_camf_matrix(
                    reader.x3f.as_ptr(),
                    c"ColorDQCamRGB".as_ptr() as *mut _,
                    3,
                    0,
                    0,
                    sys::matrix_type_t_M_FLOAT,
                    amplitudes.as_mut_ptr().cast(),
                ) != 0
                && sys::x3f_get_camf_matrix(
                    reader.x3f.as_ptr(),
                    c"TCColorModeSettings".as_ptr() as *mut _,
                    TONE_MODE_COUNT as i32,
                    TONE_PARAMETERS_PER_MODE as i32,
                    0,
                    sys::matrix_type_t_M_FLOAT,
                    settings.as_mut_ptr().cast(),
                ) != 0
        };
        if !valid || maximum != [SATURATION as u32; 3] {
            return Err(invalid("missing calibration or unverified raw scale"));
        }
        if camera
            .iter()
            .chain(&gain)
            .chain(&black)
            .chain(&settings)
            .any(|v| !v.is_finite())
            || gain.iter().any(|v| *v <= 0.0)
            || black.iter().any(|v| !(0.0..SATURATION as f64).contains(v))
            || !iso.is_finite()
            || iso <= 0.0
        {
            return Err(invalid("invalid calibration"));
        }
        let [scale, start, end, steep1, _gamma, breakpoint, steep2]: [f64;
            TONE_PARAMETERS_PER_MODE] = std::array::from_fn(|i| settings[i]);
        // The normal Standard branch uses five shape values from its CAMF row.
        // It does not use the stored gamma or a multi-axis color-mode table.
        if scale != 1.0
            || start >= end
            || steep1 <= 0.0
            || steep2 <= 0.0
            || breakpoint <= 0.0
            || breakpoint >= 1.0
            || reader
                .dng_camf_multi_axis_table("MultiAxisTable_Standard")
                .is_some()
        {
            return Err(invalid("unverified tone parameters"));
        }
        let tone = native_tone([start, end, steep1, breakpoint, steep2]);
        // Native ColorDQ uses float ISO division. The additional setup gain
        // changes its input coordinates, not its correction amplitude.
        let dq_iso = (capture as f32 / sensor as f32).min(color_dq::MAX_ISO_GAIN) as f64;
        let dq = ColorDq::new(amplitudes.map(|v| v * dq_iso))?;
        control.check()?;
        Ok(Self {
            camera,
            gain,
            black,
            iso,
            tone,
            dq,
        })
    }

    /// Add the look and tone tags using the DNG's published calibration and scale.
    /// Leave raw samples, camera calibration, exposure tags, and previews unchanged.
    pub(super) fn embed(
        &self,
        reader: &Reader,
        wb: &str,
        calibration: &ColorCalibration,
        scale: f64,
        ifd: &mut DirectoryWriter,
        control: Control<'_>,
    ) -> Result<(), Error> {
        let forward = profiles::default_forward_matrix(reader, wb, calibration)
            .ok_or_else(|| invalid("missing forward matrix"))?;
        let forward = forward.map(|v| {
            let (n, d) = profiles::srational_pair(v as f64, TAG_DENOMINATOR);
            n as f64 / d as f64
        });
        let forward = normalize_white(forward);
        let neutral = calibration.neutral_tag().map(|(n, d)| n as f64 / d as f64);
        let maximum = neutral.into_iter().fold(0.0, f64::max);
        let white = neutral.map(|v| v / maximum);
        let camera_to_romm = mat3_mul(
            &mat3_mul(&mat3_inverse(&normalize_white(ROMM)), &forward),
            &mat3_diag(&white.map(|v| 1.0 / v)),
        );
        let exposure =
            super::baseline_exposure(Some(self.iso.log2()), calibration.digital_gain_ev, scale)
                .ok_or_else(|| invalid("invalid exposure"))?;
        let (n, d) = profiles::srational_pair(exposure as f32 as f64, TAG_DENOMINATOR);
        let published_exposure = n as f64 / d as f64;
        // The LUT input follows Adobe's positive-exposure ramp. Restore this
        // DNG's published headroom and apply the native setup gain exactly once.
        let ramp_gain = 2.0_f64.powf(published_exposure.max(0.0));
        let ratio = SETUP_GAIN * 2.0_f64.powf(published_exposure) / ramp_gain;
        let sensor_scale = std::array::from_fn(|i| {
            self.gain[i] * (SATURATION as f64 - self.black[i]) / SATURATION as f64 * ratio
        });
        let adapter = mat3_mul(
            &mat3_mul(&self.camera, &mat3_diag(&sensor_scale)),
            &mat3_inverse(&camera_to_romm),
        );
        if adapter.iter().any(|v| !v.is_finite()) {
            return Err(invalid("singular coordinate mapping"));
        }
        let neutral_scale = mul(&adapter, [1.0; 3]).iter().sum::<f64>() / 3.0;
        if !neutral_scale.is_finite() || neutral_scale <= 0.0 {
            return Err(invalid("invalid neutral coordinate scale"));
        }
        // Adobe requires neutral look-table value scales to remain one.
        // Carry neutral brightness in the tone curve instead. Its input follows
        // Adobe's separate, white-preserving negative-exposure tone adjustment.
        let curve = profile_curve(&self.tone, neutral_scale, published_exposure);
        let effective_tone: Vec<f64> = (0..=SATURATION)
            .map(|i| {
                curve_at(
                    &curve,
                    exposure_tone(i as f64 / SATURATION as f64, published_exposure),
                )
            })
            .collect();
        let table = look::bake(
            &adapter,
            &ADOBE_TO_ROMM,
            &self.tone,
            &effective_tone,
            &self.dq,
            control,
        )?;
        ifd.add(
            tags::PROFILE_LOOK_TABLE_DIMS,
            Value::Long(look::DIMS.to_vec()),
        );
        ifd.add(tags::PROFILE_LOOK_TABLE_DATA, Value::Float(table));
        ifd.add(
            tags::PROFILE_LOOK_TABLE_ENCODING,
            Value::Long(vec![look::ENCODING_SRGB]),
        );
        ifd.add(tags::PROFILE_TONE_CURVE, Value::Float(curve));
        ifd.add(
            tags::DNG_BACKWARD_VERSION,
            Value::Byte(tags::DNG_VERSION_1_4_0_0.to_vec()),
        );
        Ok(())
    }
}

fn invalid(message: &str) -> Error {
    Error::InvalidData(format!("SPP Standard experiment: {message}"))
}

fn normalize_white(m: [f64; 9]) -> [f64; 9] {
    std::array::from_fn(|i| {
        m[i] * PCS_WHITE[i / 3] / m[(i / 3) * 3..(i / 3) * 3 + 3].iter().sum::<f64>()
    })
}

fn mul(m: &[f64; 9], v: [f64; 3]) -> [f64; 3] {
    // Preserve the accumulation order used by the baked reference.
    std::array::from_fn(|r| m[r * 3].mul_add(v[0], m[r * 3 + 1].mul_add(v[1], m[r * 3 + 2] * v[2])))
}

fn srgb_decode(x: f64) -> f64 {
    if x <= SRGB_ENCODED_JOIN {
        x / SRGB_SLOPE
    } else {
        ((x + SRGB_OFFSET) / SRGB_SCALE).powf(SRGB_POWER)
    }
}

fn srgb_encode(x: f64) -> f64 {
    if x <= SRGB_LINEAR_JOIN {
        x * SRGB_SLOPE
    } else {
        SRGB_SCALE * x.powf(1.0 / SRGB_POWER) - SRGB_OFFSET
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_romm_matches_adobe_white() {
        let white = mul(&normalize_white(ROMM), [1.0; 3]);
        for i in 0..3 {
            assert!((white[i] - PCS_WHITE[i]).abs() < 1e-12);
        }
    }
}
