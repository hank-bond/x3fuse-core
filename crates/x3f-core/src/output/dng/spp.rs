//! Embed a CAMF-derived color mode without changing linear camera samples.
//!
//! DP1, DP2 and DP3 Merrill share this rendering recipe. Each file supplies its
//! camera matrix and gains for the selected white balance. Its source color-space
//! property selects native sRGB or Adobe RGB routing, not the renderer's output.
//!
//! The camera adapter maps Adobe's rendering coordinates into the native tone
//! and ColorDQ input range. A separate profile curve carries neutral brightness.
//! Spatial processing, including white-balance color shading, stays upstream.
//! This recipe does not reproduce the full Sigma Photo Pro pipeline.

mod color_dq;
mod look;
mod mode;
mod tone;

use x3f_sys::{self as sys, Control};

use super::{
    color::ColorCalibration,
    metadata::{mat3_diag, mat3_inverse, mat3_mul},
    profiles, tags,
    tiff_writer::{DirectoryWriter, Value},
};
use crate::{Error, Reader, SppMode};
use color_dq::ColorDq;
use mode::ModeParameters;
use tone::{curve_at, exposure_tone, profile_curve, SATURATION};

// Rendering setup

// The supported native setup branch adds one stop before ColorDQ and tone.
// This gain belongs in rendering metadata, not in the stored camera samples.
const SETUP_GAIN: f64 = 2.0;

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

// Native source 2 selects sRGB. Select this matrix for an sRGB camera property,
// independently of the DNG renderer's output color space.
const SRGB_TO_ROMM: [f64; 9] = [
    2168.0 / MATRIX_SCALE,
    1352.0 / MATRIX_SCALE,
    576.0 / MATRIX_SCALE,
    403.0 / MATRIX_SCALE,
    3578.0 / MATRIX_SCALE,
    115.0 / MATRIX_SCALE,
    69.0 / MATRIX_SCALE,
    482.0 / MATRIX_SCALE,
    3546.0 / MATRIX_SCALE,
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

pub(super) struct SppLook {
    mode: SppMode,
    camera: [f64; 9],
    output_matrix: [f64; 9],
    gain: [f64; 3],
    black: [f64; 3],
    iso: f64,
    tone: Vec<f64>,
    dq: ColorDq,
}

impl SppLook {
    /// Capture calibration before processing changes the decoded sensor raster.
    /// Resolve Auto, Daylight or Sunlight calibration from the file's CAMF lists.
    /// Auto selects stored calibration rather than estimating a new white balance.
    pub(super) fn prepare(
        reader: &Reader,
        wb: &str,
        mode: SppMode,
        control: Control<'_>,
    ) -> Result<Self, Error> {
        control.check()?;
        if !matches!(
            reader.dng_prop("CAMMODEL").as_deref(),
            Some("SIGMA DP1 Merrill" | "SIGMA DP2 Merrill" | "SIGMA DP3 Merrill")
        ) || !matches!(wb, "Auto" | "Daylight" | "Sunlight")
        {
            return Err(invalid(
                "requires DP Merrill Auto, Daylight or Sunlight calibration",
            ));
        }
        let mut camera = reader
            .dng_camf_wb_matrix_3x3("WhiteBalanceColorCorrections", wb)
            .ok_or_else(|| invalid("missing selected white-balance matrix"))?;
        let parameters = ModeParameters::read(reader, mode)?;
        if mode != SppMode::Standard {
            // Native stage 2 right-multiplies the camera matrix by the mode matrix
            // before ColorDQ. This metadata path preserves that pointwise order
            // but does not move the mode inside upstream denoising or recovery.
            camera = mat3_mul(&camera, &parameters.matrix);
        }
        let output_matrix = source_to_romm(reader.dng_prop("COLORSPACE").as_deref())?;
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
        };
        if !valid || maximum != [SATURATION as u32; 3] {
            return Err(invalid("missing calibration or unverified raw scale"));
        }
        if camera
            .iter()
            .chain(&gain)
            .chain(&black)
            .any(|v| !v.is_finite())
            || gain.iter().any(|v| *v <= 0.0)
            || black.iter().any(|v| !(0.0..SATURATION as f64).contains(v))
            || !iso.is_finite()
            || iso <= 0.0
        {
            return Err(invalid("invalid calibration"));
        }
        let tone = parameters.tone;
        // Native ColorDQ uses float ISO division. The additional setup gain
        // changes its input coordinates, not its correction amplitude.
        let dq_iso = (capture as f32 / sensor as f32).min(color_dq::MAX_ISO_GAIN) as f64;
        let dq = ColorDq::new(amplitudes.map(|v| v * dq_iso))?;
        control.check()?;
        Ok(Self {
            mode,
            camera,
            output_matrix,
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
            &self.output_matrix,
            &self.tone,
            &effective_tone,
            &self.dq,
            control,
        )?;
        let name = std::ffi::CString::new(self.mode.as_str()).expect("mode names contain no NUL");
        ifd.add(tags::PROFILE_NAME, Value::Ascii(name.clone()));
        ifd.add(tags::AS_SHOT_PROFILE_NAME, Value::Ascii(name));
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

fn source_to_romm(space: Option<&str>) -> Result<[f64; 9], Error> {
    match space {
        Some("sRGB") => Ok(SRGB_TO_ROMM),
        Some("AdobeRGB") => Ok(ADOBE_TO_ROMM),
        _ => Err(invalid("missing or unsupported source color space")),
    }
}

fn invalid(message: &str) -> Error {
    Error::InvalidData(format!("SPP rendering: {message}"))
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
    fn source_color_property_selects_native_routing() {
        assert_eq!(source_to_romm(Some("sRGB")).unwrap(), SRGB_TO_ROMM);
        assert_eq!(source_to_romm(Some("AdobeRGB")).unwrap(), ADOBE_TO_ROMM);
        assert_ne!(SRGB_TO_ROMM, ADOBE_TO_ROMM);
        for space in [None, Some(""), Some("ProPhoto"), Some("unknown")] {
            assert!(source_to_romm(space).is_err());
        }
    }

    #[test]
    fn normalized_romm_matches_adobe_white() {
        let white = mul(&normalize_white(ROMM), [1.0; 3]);
        for i in 0..3 {
            assert!((white[i] - PCS_WHITE[i]).abs() < 1e-12);
        }
    }
}
