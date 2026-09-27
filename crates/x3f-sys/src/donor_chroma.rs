//! Estimate highlight color ratios from nearby reliable pixels, called donors.
//!
//! Read donor counts, color sums, and squared sums from the smallest grid tiles.
//! Weights fade to zero at the radius boundary. Available source layers reduce
//! the weight of donors with conflicting ratios. Matching ratios do not prove
//! that a donor belongs to the same material.

use super::{
    camera_smoothstep, signal_floor, Level, MAX_AMPLITUDE_DISAGREEMENT, MAX_VARIATION, MIN_DONORS,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct Settings {
    radius: usize,
}

impl Settings {
    // Borrow donor chromaticity, not donor brightness or reconstructed layers.
    pub(super) fn colorization(radius: usize) -> Self {
        Self { radius }
    }

    pub(super) fn estimate(
        self,
        level: &Level,
        position: [f64; 2],
        samples: [f64; 3],
        mask: [u8; 3],
        noise: [f64; 3],
    ) -> Option<([f64; 3], f64)> {
        let [row, col] = position;
        if level.rows == 0
            || level.cols == 0
            || level.tile_size == 0
            || self.radius == 0
            || !row.is_finite()
            || !col.is_finite()
            || row < 0.0
            || col < 0.0
            || samples.iter().any(|v| !v.is_finite())
        {
            return None;
        }
        let radius = self.radius as f64;
        let size = level.tile_size as f64;
        // All centroids with nonzero kernel weight are inside these bounds.
        // A tile leaving the scan window already has zero kernel weight.
        let first_row = ((row - radius).max(0.0) / size).floor() as usize;
        let first_col = ((col - radius).max(0.0) / size).floor() as usize;
        let last_row = (((row + radius) / size).floor() as usize).min(level.rows - 1);
        let last_col = (((col + radius) / size).floor() as usize).min(level.cols - 1);
        let mut sum = [0.0; 3];
        let mut squares = [0.0; 3];
        let mut support = 0.0;
        for r in first_row..=last_row {
            for c in first_col..=last_col {
                let tile = &level.tiles[r * level.cols + c];
                if tile.count == 0 {
                    continue;
                }
                let count = tile.count as f64;
                let dr = tile.row_sum / count - row;
                let dc = tile.col_sum / count - col;
                let mut weight = compact((dr * dr + dc * dc) / (radius * radius));
                if weight == 0.0 {
                    continue;
                }
                weight *= compatibility(samples, tile.sum.map(|v| v / count), mask, noise);
                support += weight * count;
                for channel in 0..3 {
                    sum[channel] += weight * tile.sum[channel];
                    squares[channel] += weight * tile.square_sum[channel];
                }
            }
        }
        if !support.is_finite() || support <= 0.0 {
            return None;
        }
        let chroma = sum.map(|v| v / support);
        if chroma.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return None;
        }
        let mut variation = 0.0_f64;
        for channel in 0..3 {
            let variance = (squares[channel] / support - chroma[channel].powi(2)).max(0.0);
            variation = variation.max(variance.sqrt() / chroma[channel]);
        }
        // Include color differences within each tile, not just between tile means.
        // Averaging mixed colors must not make them look more consistent.
        let coherence = camera_smoothstep(1.0 - variation / MAX_VARIATION);
        let confidence = camera_smoothstep(support / MIN_DONORS) * coherence;
        Some((chroma, confidence))
    }
}

/// The `q` parameter is squared distance divided by squared radius.
/// The function value and slope are zero at `q = 1`.
fn compact(q: f64) -> f64 {
    (1.0 - q).clamp(0.0, 1.0).powi(2)
}

fn compatibility(s: [f64; 3], chroma: [f64; 3], mask: [u8; 3], noise: [f64; 3]) -> f64 {
    let trust: [f64; 3] = std::array::from_fn(|c| {
        let floor = signal_floor(noise[c]);
        (mask[c] as f64 / 255.0).powi(2) * camera_smoothstep((s[c] - floor) / floor)
    });
    let mut weight = 1.0;
    for a in 0..3 {
        for b in a + 1..3 {
            let pair_trust = trust[a] * trust[b];
            if pair_trust == 0.0 {
                continue;
            }
            // Relative disagreement between the amplitudes implied by
            // each survivor, without dividing by a small chroma component.
            let left = s[a] * chroma[b];
            let right = s[b] * chroma[a];
            let denominator = left + right;
            if denominator <= 0.0 || !denominator.is_finite() {
                return 0.0;
            }
            let disagreement = 2.0 * (left - right).abs() / denominator;
            let agreement = compact((disagreement / MAX_AMPLITUDE_DISAGREEMENT).powi(2));
            weight *= 1.0 - pair_trust * (1.0 - agreement);
        }
    }
    weight
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight_recovery::{LocalRecovery, SensorReliability};

    fn level(points: &[(usize, usize, [f64; 3], usize)]) -> Level {
        let mut level = Level::new(16, 16, 16).unwrap();
        for &(row, col, chroma, count) in points {
            for _ in 0..count {
                level.tiles[(row / 16) * level.cols + col / 16].add(row, col, chroma);
            }
        }
        level
    }

    fn estimate(level: &Level, position: [f64; 2]) -> Option<([f64; 3], f64)> {
        // One survivor cannot constrain donor ratios, so compatibility is one.
        Settings::colorization(64).estimate(
            level,
            position,
            [0.3, 0.6, 1.0],
            [255, 0, 0],
            [0.001; 3],
        )
    }

    #[test]
    fn repaired_pixels_cannot_supply_color_but_keep_source_reliability() {
        let mut reliability = SensorReliability::new(16, 16, [0.001; 3]).unwrap();
        reliability.data[8 * 16 + 8] = [255, 255, 0];
        reliability.repair_marked.fill(true);
        let masks = reliability.data.clone();
        let model = LocalRecovery::build(reliability, |_, _| [0.2, 0.3, 0.5], None).unwrap();
        for row in 0..16 {
            for col in 0..16 {
                assert_eq!(model.mask(row, col), masks[row * 16 + col]);
            }
        }
        assert!(model
            .colorization_direction(8, 8, [0.2, 0.3, 0.8], 256)
            .is_none());
    }

    #[test]
    fn repaired_color_is_excluded_without_discarding_real_donors() {
        let good = [0.2, 0.3, 0.5];
        let mut reliability = SensorReliability::new(16, 16, [0.001; 3]).unwrap();
        for row in 0..16 {
            for col in 8..16 {
                reliability.repair_marked[row * 16 + col] = true;
            }
        }
        reliability.data[8 * 16 + 8] = [255, 0, 0];
        let model = LocalRecovery::build(
            reliability,
            |_, col| if col < 8 { good } else { [0.4, 0.2, 0.4] },
            None,
        )
        .unwrap();
        let (chroma, confidence) = model
            .colorization_direction(8, 8, [0.2, 0.8, 0.9], 256)
            .unwrap();
        for c in 0..3 {
            assert!((chroma[c] - good[c]).abs() < 1e-12);
        }
        assert!(confidence > 0.99);
    }

    #[test]
    fn mismatched_repair_provenance_is_rejected() {
        let mut reliability = SensorReliability::new(1, 2, [0.001; 3]).unwrap();
        reliability.repair_marked.pop();
        assert!(LocalRecovery::build(reliability, |_, _| [0.2; 3], None).is_none());
    }

    #[test]
    fn constant_color_is_preserved() {
        let p = [0.2, 0.3, 0.5];
        let level = level(&[(64, 64, p, 32), (64, 96, p, 16)]);
        for col in [63.9, 64.0, 64.1, 79.9, 80.0, 80.1, 96.0] {
            let (chroma, confidence) = estimate(&level, [64.0, col]).unwrap();
            assert!(chroma
                .into_iter()
                .zip(p)
                .all(|(a, b)| (a - b).abs() < 1e-12));
            assert!(confidence > 0.99);
        }
    }

    #[test]
    fn crossing_tile_and_scan_window_boundaries_has_no_finite_jump() {
        let level = level(&[
            (64, 8, [0.2, 0.3, 0.5], 32),
            (64, 72, [0.21, 0.3, 0.49], 32),
            (64, 120, [0.19, 0.3, 0.51], 32),
        ]);
        for boundary in [64.0, 72.0, 80.0, 96.0, 128.0] {
            let (left, lc) = estimate(&level, [64.0, boundary - 1e-7]).unwrap();
            let (right, rc) = estimate(&level, [64.0, boundary + 1e-7]).unwrap();
            assert!(left
                .into_iter()
                .zip(right)
                .all(|(a, b)| (a - b).abs() < 1e-8));
            assert!((lc - rc).abs() < 1e-6);
        }
    }

    #[test]
    fn support_fades_to_zero_instead_of_an_eight_donor_switch() {
        let level = level(&[(64, 64, [0.2, 0.3, 0.5], 8)]);
        let (_, near) = estimate(&level, [64.0, 100.0]).unwrap();
        let (_, edge) = estimate(&level, [64.0, 128.0 - 1e-5]).unwrap();
        assert!(near > 0.0 && near < 1.0);
        assert!(edge >= 0.0 && edge < 1e-15);
        assert!(estimate(&level, [64.0, 128.0]).is_none());
    }

    #[test]
    fn guidance_prefers_survivor_compatible_donors() {
        let good = [0.2, 0.4, 0.4];
        let level = level(&[(64, 56, good, 32), (64, 72, [0.4, 0.2, 0.4], 32)]);
        let (single_survivor, _) = estimate(&level, [64.0, 64.0]).unwrap();
        let guided = Settings::colorization(64);
        let (matched, confidence) = guided
            .estimate(
                &level,
                [64.0, 64.0],
                [0.3, 0.6, 1.0],
                [255, 255, 0],
                [0.001; 3],
            )
            .unwrap();
        assert!((single_survivor[0] - 0.3).abs() < 1e-12);
        assert!(matched
            .into_iter()
            .zip(good)
            .all(|(a, b)| (a - b).abs() < 1e-12));
        assert!(confidence > 0.99);
    }

    #[test]
    fn clipping_and_single_survivor_do_not_invent_guidance() {
        let chroma = [0.2, 0.4, 0.4];
        let a = compatibility([0.3, 0.6, 1.0], chroma, [255, 255, 0], [0.001; 3]);
        let b = compatibility([0.3, 0.6, 100.0], chroma, [255, 255, 0], [0.001; 3]);
        assert_eq!(a, b);
        assert_eq!(
            compatibility([0.3, 3.0, 10.0], chroma, [255, 0, 0], [0.001; 3]),
            1.0
        );
        let low = compatibility([0.3, 0.6, 1.0], chroma, [255, 255, 1], [0.001; 3]);
        assert!((a - low).abs() < 0.00004);
    }

    #[test]
    fn within_tile_conflict_is_not_hidden_by_averaging() {
        let level = level(&[(64, 64, [0.2, 0.4, 0.4], 32), (64, 65, [0.4, 0.2, 0.4], 32)]);
        let (_, confidence) = estimate(&level, [64.0, 64.0]).unwrap();
        assert_eq!(confidence, 0.0);
        assert!(estimate(&Level::new(1, 1, 16).unwrap(), [0.0, 0.0]).is_none());
    }

    #[test]
    fn colorization_can_reach_broader_support_without_reconstructing_layers() {
        let source = [0.3, 0.5, 0.8];
        let mut reliability = SensorReliability::new(128, 256, [0.001; 3]).unwrap();
        reliability.data[64 * 256 + 240] = [255, 255, 0];
        let model = LocalRecovery::build(
            reliability,
            |_, col| {
                if col < 80 {
                    source
                } else {
                    [0.0; 3]
                }
            },
            None,
        )
        .unwrap();
        let clipped = [0.3, 0.5, 0.4];
        assert!(model
            .colorization_direction(64, 240, clipped, 128)
            .is_none());
        let (direction, confidence) = model.colorization_direction(64, 240, clipped, 256).unwrap();
        for c in 0..3 {
            assert!((direction[c] - source[c] / 1.6).abs() < 1e-12);
        }
        assert!(confidence > 0.99);
        assert!(model.colorization_direction(128, 0, clipped, 256).is_none());
    }
}
