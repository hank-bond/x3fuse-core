//! Continuous Merrill highlight color, independent of the frozen tone solve.
//!
//! An eight-pixel graph combines intact boundaries and reliable same-site layer
//! ratios. Missing color has a soft calibrated-neutral prior. A balanced pair
//! alone does not establish white: its missing third-layer relationship can
//! equally belong to a colored surface. No fixed donor radius or scene classes.
use super::{tone_anchor, DngCtx, LocalRecovery};
use crate::{x3f_calc_spatial_gain, Control, Error::InvalidData};
use std::{cmp::Ordering, collections::BinaryHeap};
const STEP: usize = 8;
const NONE: usize = usize::MAX;
const DATA_WEIGHT: f64 = 4.0;
const PAIR_SCALE: f64 = 0.035;

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
fn evidence(m: [f64; 3], mask: [u8; 3], repaired: bool) -> ([f64; 3], [f64; 3]) {
    let q: [f64; 3] = std::array::from_fn(|c| {
        if repaired || !m[c].is_finite() {
            0.0
        } else {
            (mask[c] as f64 / 255.0).powi(4) * smooth((m[c] - 0.002) / 0.018)
        }
    });
    let l = m.map(|v| v.max(1e-12).ln());
    (
        [l[0] - l[1], l[0] - l[2], l[1] - l[2]],
        [q[0] * q[1], q[0] * q[2], q[1] * q[2]],
    )
}
#[derive(Clone, Copy, Default)]
struct Guide {
    pair: [f64; 3],
    q: [f64; 3],
    tone: f64,
}
fn affinity(a: Guide, b: Guide) -> f64 {
    if !a.tone.is_finite() || !b.tone.is_finite() {
        return 0.0;
    }
    let t = (a.tone - b.tone) / (0.25 * std::f64::consts::LN_2);
    let disagreement = (0..3)
        .map(|k| a.q[k].min(b.q[k]) * ((a.pair[k] - b.pair[k]) / PAIR_SCALE).powi(2))
        .fold(0.0_f64, f64::max);
    let w = (-0.5 * (t * t + disagreement)).exp();
    if w < 1e-4 {
        0.0
    } else {
        w
    }
}
/// Limit only N's neutralward move, not M's field or the evidence weights.
/// The bound allows one existing agreement scale of additional weighted error;
/// this is a conservative heuristic, not a calibrated noise confidence interval.
fn retained_neutral_move(before: [f64; 2], proposed: [f64; 2], g: Guide) -> f64 {
    let ratios = |v: [f64; 2]| [v[0] - v[1], v[0], v[1]];
    let a = ratios(before);
    let b = ratios(proposed);
    let mut retained: f64 = 1.0;
    for k in 0..3 {
        if g.q[k] <= 0.0 {
            continue;
        }
        let trust = g.q[k].sqrt();
        let old_error = trust * (a[k] - g.pair[k]);
        let new_error = trust * (b[k] - g.pair[k]);
        let bound = old_error.abs() + PAIR_SCALE;
        if new_error.abs() > bound {
            let edge = bound.copysign(new_error);
            retained = retained.min((edge - old_error) / (new_error - old_error));
        }
    }
    retained.clamp(0.0, 1.0)
}
fn neutral_weight(g: Guide) -> f64 {
    let trust = g.q.into_iter().fold(0.0_f64, f64::max).clamp(0.0, 1.0);
    0.0001 + 0.04 * (1.0 - trust).powi(2)
}
#[derive(Clone, Copy)]
struct ColorPrior {
    neutral: [f64; 2],
    metric: [f64; 3],
}
impl ColorPrior {
    /// A solved field can disagree with native evidence even when its coarse
    /// guide averages agree. Borrow only the compatible part of that proposal;
    /// keep the existing uncertainty reference, not a semantic white decision.
    fn compatible_target(self, field: [f64; 2], guide: Guide) -> [f64; 2] {
        let proposal = Guide {
            pair: [field[0] - field[1], field[0], field[1]],
            q: [1.0; 3],
            tone: guide.tone,
        };
        let confidence = affinity(guide, proposal);
        std::array::from_fn(|k| self.neutral[k] + confidence * (field[k] - self.neutral[k]))
    }
    fn new(p: [f64; 3], m: [f64; 9]) -> crate::Result<Self> {
        let rgb: [f64; 3] = std::array::from_fn(|r| (0..3).map(|c| m[3 * r + c] * p[c]).sum());
        if p.iter()
            .chain(rgb.iter())
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err(InvalidData("invalid neutral color metric"));
        }
        // Jacobian of log(R/G), log(B/G) at calibrated neutral. The raw
        // layer axes are not perceptually orthogonal; equal BMT is not white.
        let j: [[f64; 2]; 2] = [0, 2]
            .map(|r| std::array::from_fn(|c| p[c] * (m[3 * r + c] / rgb[r] - m[3 + c] / rgb[1])));
        let a = j[0][0] * j[0][0] + j[1][0] * j[1][0];
        let b = j[0][1] * j[0][1] + j[1][1] * j[1][1];
        let c = j[0][0] * j[0][1] + j[1][0] * j[1][1];
        let minimum = 0.5 * (a + b - ((a - b) * (a - b) + 4.0 * c * c).sqrt());
        if !minimum.is_finite() || minimum <= 1e-9 {
            return Err(InvalidData("singular neutral color metric"));
        }
        Ok(Self {
            neutral: [(p[0] / p[2]).ln(), (p[1] / p[2]).ln()],
            metric: [a / minimum, b / minimum, c / minimum],
        })
    }
    fn add(self, block: &mut [f64; 3], rhs: &mut [f64; 2], target: [f64; 2], weight: f64) {
        for k in 0..3 {
            block[k] += weight * self.metric[k];
        }
        rhs[0] += weight * (self.metric[0] * target[0] + self.metric[2] * target[1]);
        rhs[1] += weight * (self.metric[2] * target[0] + self.metric[1] * target[1]);
    }
}
fn surface_agreement(a: Guide, b: Guide) -> f64 {
    if !a.tone.is_finite() || !b.tone.is_finite() {
        return 0.0;
    }
    let tone = (a.tone - b.tone) / (0.75 * std::f64::consts::LN_2);
    let ratio = (0..3)
        .map(|k| a.q[k].min(b.q[k]) * ((a.pair[k] - b.pair[k]) / 0.12).powi(2))
        .fold(0.0_f64, f64::max);
    (-0.5 * (tone * tone + ratio)).exp()
}
#[derive(Clone)]
struct Node {
    guide: Guide,
    fixed: bool,
    color: [f64; 2],
    distance: f64,
    origin: usize,
    affected: bool,
    region: usize,
}
impl Default for Node {
    fn default() -> Self {
        Self {
            guide: Guide::default(),
            fixed: false,
            color: [0.0; 2],
            distance: f64::INFINITY,
            origin: NONE,
            affected: false,
            region: NONE,
        }
    }
}
#[derive(Clone, Copy)]
struct Visit {
    distance: f64,
    index: usize,
    origin: usize,
}
impl PartialEq for Visit {
    fn eq(&self, b: &Self) -> bool {
        self.distance == b.distance && self.index == b.index && self.origin == b.origin
    }
}
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Visit {
    fn cmp(&self, b: &Self) -> Ordering {
        b.distance
            .total_cmp(&self.distance)
            .then_with(|| b.index.cmp(&self.index))
            .then_with(|| b.origin.cmp(&self.origin))
    }
}
#[derive(Clone)]
struct Equation {
    // One camera-calibrated metric shared by all spatial edges in this solve.
    spatial_metric: [f64; 3],
    block: [f64; 3],
    neighbors: [usize; 4],
    weights: [f64; 4],
}
fn inverse(a: [f64; 3], x: [f64; 2]) -> [f64; 2] {
    let det = a[0] * a[1] - a[2] * a[2];
    [
        (a[1] * x[0] - a[2] * x[1]) / det,
        (a[0] * x[1] - a[2] * x[0]) / det,
    ]
}
fn product(rows: &[Equation], x: &[[f64; 2]], out: &mut [[f64; 2]]) {
    for (i, e) in rows.iter().enumerate() {
        let [a, b, c] = e.block;
        out[i] = [a * x[i][0] + c * x[i][1], c * x[i][0] + b * x[i][1]];
        for k in 0..4 {
            if e.neighbors[k] != NONE {
                let z = x[e.neighbors[k]];
                let [a, b, c] = e.spatial_metric;
                out[i][0] -= e.weights[k] * (a * z[0] + c * z[1]);
                out[i][1] -= e.weights[k] * (c * z[0] + b * z[1]);
            }
        }
    }
}
fn norm(x: &[[f64; 2]]) -> f64 {
    x.iter()
        .flatten()
        .map(|v| {
            if v.is_finite() {
                v.abs()
            } else {
                f64::INFINITY
            }
        })
        .fold(0.0_f64, f64::max)
}
fn dot(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(a, b)| a * b)
        .sum()
}
fn solve(
    rows: &[Equation],
    rhs: &[[f64; 2]],
    x: &mut [[f64; 2]],
    control: Control<'_>,
) -> crate::Result<(usize, f64)> {
    if rows.is_empty() {
        return Ok((0, 0.0));
    }
    if rows.iter().any(|e| {
        e.block.iter().any(|v| !v.is_finite())
            || e.block[0] * e.block[1] - e.block[2] * e.block[2] <= 0.0
    }) {
        return Err(InvalidData("color field preconditioner not positive"));
    }
    let tolerance = 1e-9 * norm(rhs).max(1.0);
    if !tolerance.is_finite() {
        return Err(InvalidData("color field RHS not finite"));
    }
    let mut ax = vec![[0.0; 2]; x.len()];
    product(rows, x, &mut ax);
    let mut residual: Vec<[f64; 2]> = rhs
        .iter()
        .zip(&ax)
        .map(|(a, b)| [a[0] - b[0], a[1] - b[1]])
        .collect();
    let mut z: Vec<_> = rows
        .iter()
        .zip(&residual)
        .map(|(e, r)| inverse(e.block, *r))
        .collect();
    let mut direction = z.clone();
    let mut rz = dot(&residual, &z);
    for iteration in 0..=4000 {
        control.check()?;
        if norm(&residual) <= tolerance {
            product(rows, x, &mut ax);
            let actual = rhs
                .iter()
                .flatten()
                .zip(ax.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f64, f64::max);
            if !norm(&ax).is_finite()
                || !actual.is_finite()
                || actual > tolerance
                || !norm(x).is_finite()
            {
                return Err(InvalidData("color field true residual failed"));
            }
            return Ok((iteration, actual));
        }
        if iteration == 4000 {
            return Err(InvalidData("color field did not converge"));
        }
        product(rows, &direction, &mut ax);
        let denominator = dot(&direction, &ax);
        if denominator <= 0.0 || !denominator.is_finite() || !rz.is_finite() {
            return Err(InvalidData("color field solver breakdown"));
        }
        let alpha = rz / denominator;
        for i in 0..x.len() {
            for ch in 0..2 {
                x[i][ch] += alpha * direction[i][ch];
                residual[i][ch] -= alpha * ax[i][ch];
            }
            z[i] = inverse(rows[i].block, residual[i]);
        }
        let next = dot(&residual, &z);
        let beta = next / rz;
        for i in 0..x.len() {
            for ch in 0..2 {
                direction[i][ch] = z[i][ch] + beta * direction[i][ch];
            }
        }
        rz = next;
    }
    unreachable!()
}
fn data_equation(g: Guide) -> ([f64; 3], [f64; 2]) {
    let [a, b, c] = g.q.map(|v| v * DATA_WEIGHT);
    (
        [a + b, a + c, -a],
        [
            a * g.pair[0] + b * g.pair[1],
            -a * g.pair[0] + c * g.pair[2],
        ],
    )
}
fn direction(uv: [f64; 2], y: [f64; 3]) -> Option<[f64; 3]> {
    if uv.iter().any(|v| !v.is_finite() || v.abs() > 30.0) {
        return None;
    }
    let d = [uv[0].exp(), uv[1].exp(), 1.0];
    let lum = dot3(y, d);
    (lum.is_finite() && lum > 1e-12).then(|| d.map(|v| v / lum))
}

pub(super) struct Field {
    rows: usize,
    cols: usize,
    nodes: Vec<Node>,
    horizontal: Vec<f64>,
    vertical: Vec<f64>,
    prior: ColorPrior,
}
impl Field {
    fn neighbors(&self, i: usize) -> [(usize, f64); 4] {
        let r = i / self.cols;
        let c = i % self.cols;
        [
            if r > 0 {
                (i - self.cols, self.vertical[i - self.cols])
            } else {
                (NONE, 0.0)
            },
            if c > 0 {
                (i - 1, self.horizontal[i - 1])
            } else {
                (NONE, 0.0)
            },
            if r + 1 < self.rows {
                (i + self.cols, self.vertical[i])
            } else {
                (NONE, 0.0)
            },
            if c + 1 < self.cols {
                (i + 1, self.horizontal[i])
            } else {
                (NONE, 0.0)
            },
        ]
    }
    fn condition_boundaries(&mut self, control: Control<'_>) -> crate::Result<()> {
        let mut seen = vec![false; self.nodes.len()];
        for i in 0..self.nodes.len() {
            if seen[i] || !self.nodes[i].affected {
                continue;
            }
            control.check()?;
            let mut component = vec![i];
            seen[i] = true;
            let mut peak = i;
            let mut k = 0;
            while k < component.len() {
                if k % 65536 == 0 {
                    control.check()?;
                }
                let n = component[k];
                k += 1;
                if self.nodes[n].guide.tone > self.nodes[peak].guide.tone {
                    peak = n;
                }
                for (j, w) in self.neighbors(n) {
                    if j != NONE && w > 0.0 && self.nodes[j].affected && !seen[j] {
                        seen[j] = true;
                        component.push(j);
                    }
                }
            }
            for n in component {
                self.nodes[n].region = peak;
            }
        }
        for i in 0..self.nodes.len() {
            if i % 65536 == 0 {
                control.check()?;
            }
            for horizontal in [true, false] {
                let j = if horizontal {
                    if i % self.cols + 1 >= self.cols {
                        continue;
                    }
                    i + 1
                } else {
                    if i / self.cols + 1 >= self.rows {
                        continue;
                    }
                    i + self.cols
                };
                let a = &self.nodes[i];
                let b = &self.nodes[j];
                let gate = if (!a.fixed && !a.affected) || (!b.fixed && !b.affected) {
                    0.0
                } else if a.affected && b.fixed {
                    surface_agreement(self.nodes[a.region].guide, b.guide)
                } else if b.affected && a.fixed {
                    surface_agreement(self.nodes[b.region].guide, a.guide)
                } else {
                    1.0
                };
                if horizontal {
                    self.horizontal[i] *= gate;
                } else {
                    self.vertical[i] *= gate;
                }
            }
        }
        Ok(())
    }
    fn connect(&mut self, control: Control<'_>) -> crate::Result<()> {
        for i in 0..self.nodes.len() {
            if i % 65536 == 0 {
                control.check()?;
            }
            if i % self.cols + 1 < self.cols {
                self.horizontal[i] = affinity(self.nodes[i].guide, self.nodes[i + 1].guide);
            }
            if i / self.cols + 1 < self.rows {
                self.vertical[i] = affinity(self.nodes[i].guide, self.nodes[i + self.cols].guide);
            }
        }
        self.condition_boundaries(control)?;
        let mut queue = BinaryHeap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.fixed {
                queue.push(Visit {
                    distance: 0.0,
                    index: i,
                    origin: i,
                });
            }
        }
        let mut count = 0;
        while let Some(v) = queue.pop() {
            count += 1;
            if count % 65536 == 0 {
                control.check()?;
            }
            if self.nodes[v.index].distance <= v.distance {
                continue;
            }
            self.nodes[v.index].distance = v.distance;
            self.nodes[v.index].origin = v.origin;
            for (j, w) in self.neighbors(v.index) {
                if j != NONE && w > 0.0 {
                    let distance = v.distance + STEP as f64 / w.sqrt();
                    if distance < self.nodes[j].distance {
                        queue.push(Visit {
                            distance,
                            index: j,
                            origin: v.origin,
                        });
                    }
                }
            }
        }
        Ok(())
    }
    fn reconstruct(&mut self, control: Control<'_>) -> crate::Result<(usize, usize, f64)> {
        self.connect(control)?;
        let mut index = vec![NONE; self.nodes.len()];
        let mut active = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.fixed && n.affected {
                index[i] = active.len();
                active.push(i);
            }
        }
        let mut equations = Vec::with_capacity(active.len());
        let mut rhs = Vec::with_capacity(active.len());
        let mut x = Vec::with_capacity(active.len());
        for &i in &active {
            let n = &self.nodes[i];
            let (mut block, mut target) = data_equation(n.guide);
            self.prior.add(
                &mut block,
                &mut target,
                self.prior.neutral,
                neutral_weight(n.guide),
            );
            let mut neighbors = [NONE; 4];
            let mut weights = [0.0; 4];
            for (k, (j, w)) in self.neighbors(i).into_iter().enumerate() {
                if j == NONE || w == 0.0 {
                    continue;
                }
                // Measure spatial differences in the same calibrated geometry
                // as the neutral prior, including fixed-boundary contributions.
                self.prior.add(
                    &mut block,
                    &mut target,
                    if self.nodes[j].fixed {
                        self.nodes[j].color
                    } else {
                        [0.0; 2]
                    },
                    w,
                );
                if !self.nodes[j].fixed {
                    assert_ne!(index[j], NONE);
                    neighbors[k] = index[j];
                    weights[k] = w;
                }
            }
            equations.push(Equation {
                spatial_metric: self.prior.metric,
                block,
                neighbors,
                weights,
            });
            rhs.push(target);
            x.push(if n.origin != NONE {
                self.nodes[n.origin].color
            } else {
                self.prior.neutral
            });
        }
        let (iterations, residual) = solve(&equations, &rhs, &mut x, control)?;
        for (k, &i) in active.iter().enumerate() {
            self.nodes[i].color = x[k];
        }
        Ok((active.len(), iterations, residual))
    }
    /// Read-only access to the same post-repair, post-denoise source as the
    /// headroom/encoding passes. Build once before either pass overwrites pixels.
    pub unsafe fn build(
        ctx: &DngCtx<'_>,
        model: &LocalRecovery,
        data: &[u16],
        stride: usize,
        control: Control<'_>,
    ) -> crate::Result<Self> {
        let start = std::time::Instant::now();
        let rows = (ctx.rows as usize).div_ceil(STEP);
        let cols = (ctx.cols as usize).div_ceil(STEP);
        let mut nodes = vec![Node::default(); rows * cols];
        let neutral = unsafe { *(ctx.prior as *const [f64; 3]) };
        let y = ctx
            .camera_y
            .ok_or(InvalidData("color field requires Merrill Y"))?;
        for r in 0..rows {
            control.check()?;
            for c in 0..cols {
                let n = &mut nodes[r * cols + c];
                let mut count = 0.0;
                let mut tones = 0.0;
                let mut donors = 0.0;
                let mut stressed = false;
                for row in r * STEP..((r + 1) * STEP).min(ctx.rows as usize) {
                    for col in c * STEP..((c + 1) * STEP).min(ctx.cols as usize) {
                        let off = row * stride + col * ctx.channels;
                        let measured: [f64; 3] = std::array::from_fn(|ch| {
                            let v = (data[off + ch] as f64 - ctx.black[ch])
                                / (ctx.white[ch] as f64 - ctx.black[ch]);
                            v * unsafe {
                                x3f_calc_spatial_gain(
                                    ctx.sgain,
                                    ctx.sgain_num,
                                    row as i32,
                                    col as i32,
                                    ch as i32,
                                    ctx.rows,
                                    ctx.cols,
                                )
                            }
                        });
                        let mask = model.mask(row, col);
                        let repaired = model.repaired_site(row, col);
                        n.affected |= mask != [255; 3];
                        let mut tone = tone_anchor::recover_severe(measured, mask, neutral, y);
                        if let Some(g) = ctx.gradient {
                            g.apply(row, col, &mut tone, measured, neutral);
                        }
                        let target = if tone.available {
                            tone.amplitude * dot3(y, neutral)
                        } else {
                            dot3(y, measured)
                        };
                        if target.is_finite() && target > 1e-9 {
                            n.guide.tone += target.ln();
                            tones += 1.0;
                        }
                        let (pair, q) = evidence(measured, mask, repaired);
                        for k in 0..3 {
                            n.guide.pair[k] += q[k] * pair[k];
                            n.guide.q[k] += q[k];
                        }
                        count += 1.0;
                        if !repaired {
                            stressed |= mask != [255; 3];
                            if mask == [255; 3]
                                && measured.iter().all(|v| v.is_finite() && *v > 0.02)
                                && dot3(y, measured) > 1e-9
                            {
                                n.color[0] += pair[1];
                                n.color[1] += pair[2];
                                donors += 1.0;
                            }
                        }
                    }
                }
                n.guide.tone = if tones > 0.0 {
                    n.guide.tone / tones
                } else {
                    f64::NAN
                };
                for k in 0..3 {
                    if n.guide.q[k] > 0.0 {
                        n.guide.pair[k] /= n.guide.q[k];
                    }
                    n.guide.q[k] /= count;
                }
                n.fixed = !stressed && donors >= 8.0 && n.guide.tone.is_finite();
                if n.fixed {
                    n.affected = false;
                }
                if donors > 0.0 {
                    n.color = n.color.map(|v| v / donors);
                }
            }
        }
        let size = nodes.len();
        let mut result = Self {
            rows,
            cols,
            nodes,
            horizontal: vec![0.0; size],
            vertical: vec![0.0; size],
            prior: ColorPrior::new(neutral, unsafe { *(ctx.conv_matrix as *const [f64; 9]) })?,
        };
        let (unknowns, iterations, residual) = result.reconstruct(control)?;
        let anchors = result.nodes.iter().filter(|n| n.fixed).count();
        let donorless = result
            .nodes
            .iter()
            .filter(|n| n.affected && !n.distance.is_finite())
            .count();
        unsafe {
            crate::x3f_printf(crate::x3f_verbosity_t_DEBUG,c"DNG_RECOVERY_COLOR_FIELD nodes=%zu anchors=%zu unknowns=%zu donorless=%zu iterations=%zu residual=%.17e elapsed_s=%.3f\n".as_ptr(),size,anchors,unknowns,donorless,iterations,residual,start.elapsed().as_secs_f64());
        }
        Ok(result)
    }
    /// Estimate color only. Repaired sites and a single surviving layer supply
    /// no independent ratio. Directions are normalized to camera-PCS Y = 1.
    pub fn estimate(
        &self,
        row: usize,
        col: usize,
        measured: [f64; 3],
        mask: [u8; 3],
        repaired: bool,
        target_y: f64,
        y: [f64; 3],
    ) -> Option<[f64; 3]> {
        if !target_y.is_finite() || target_y <= 0.0 {
            return None;
        }
        let (pair, q) = evidence(measured, mask, repaired);
        let guide = Guide {
            pair,
            q,
            tone: target_y.ln(),
        };
        let rr = (row as f64 + 0.5) / STEP as f64 - 0.5;
        let cc = (col as f64 + 0.5) / STEP as f64 - 0.5;
        let mut sum = [0.0; 2];
        let mut total = 0.0;
        let mut distance = 0.0;
        for r in rr.floor() as isize..=rr.floor() as isize + 1 {
            for c in cc.floor() as isize..=cc.floor() as isize + 1 {
                if r < 0 || c < 0 || r as usize >= self.rows || c as usize >= self.cols {
                    continue;
                }
                let n = &self.nodes[r as usize * self.cols + c as usize];
                if !n.distance.is_finite() && !n.affected {
                    continue;
                }
                let w = (1.0 - (rr - r as f64).abs())
                    * (1.0 - (cc - c as f64).abs())
                    * affinity(guide, n.guide);
                if w <= 0.0 {
                    continue;
                }
                total += w;
                for (ch, value) in sum.iter_mut().enumerate() {
                    *value += w * n.color[ch];
                }
                distance += w * n.distance;
            }
        }
        let p = self.prior;
        let (prior, distance) = if total > 1e-12 {
            (sum.map(|v| v / total), distance / total)
        } else {
            (p.neutral, f64::INFINITY)
        };
        let proposed_prior = p.compatible_target(prior, guide);
        let weight = 0.25 + 1.75 * (-distance / 256.0).exp();
        let (mut block, mut rhs) = data_equation(guide);
        p.add(&mut block, &mut rhs, proposed_prior, weight);
        let proposed = inverse(block, rhs);
        let (mut old_block, mut old_rhs) = data_equation(guide);
        p.add(&mut old_block, &mut old_rhs, prior, weight);
        let before = inverse(old_block, old_rhs);
        let retained = retained_neutral_move(before, proposed, guide);
        // The fit is affine in its prior. Limit both consistently, including
        // the existing invalid-ray fallback. Keep unbounded N exactly as-is.
        let (fit, prior) = if retained == 1.0 {
            (proposed, proposed_prior)
        } else {
            (
                std::array::from_fn(|k| before[k] + retained * (proposed[k] - before[k])),
                std::array::from_fn(|k| prior[k] + retained * (proposed_prior[k] - prior[k])),
            )
        };
        direction(fit, y)
            .or_else(|| direction(prior, y))
            .or_else(|| direction(p.neutral, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const P: [f64; 3] = [0.284206, 0.604436, 1.0];
    const Y: [f64; 3] = [-2.6575486148565814, 3.313530567081604, -0.2476];
    const M: [f64; 9] = [
        4.45774, -1.90728, 0.581922, -4.22818, 4.30012, -0.701463, 3.1376, -5.81127, 3.31683,
    ];
    fn line(length: usize) -> Field {
        let prior = ColorPrior::new(P, M).unwrap();
        let g = Guide {
            tone: 0.0,
            ..Guide::default()
        };
        let mut f = Field {
            rows: 1,
            cols: length,
            nodes: vec![
                Node {
                    guide: g,
                    affected: true,
                    ..Node::default()
                };
                length
            ],
            horizontal: vec![0.0; length],
            vertical: vec![0.0; length],
            prior,
        };
        f.nodes[0].fixed = true;
        f.nodes[0].affected = false;
        f.nodes[0].color = prior.neutral;
        f
    }
    #[test]
    fn compatible_non_neutral_color_is_not_rejected_by_a_balanced_pair() {
        let p = ColorPrior::new(P, M).unwrap();
        let field = [p.neutral[0] + 0.3, p.neutral[1] + 0.3];
        let g = Guide {
            pair: [field[0] - field[1], 0.0, 0.0],
            q: [1.0, 0.0, 0.0],
            tone: 0.0,
        };
        assert_eq!(p.compatible_target(field, g), field);
        assert_ne!(field, p.neutral);
    }
    #[test]
    fn only_trusted_conflicting_pairs_can_decline_a_borrowed_color() {
        let p = ColorPrior::new(P, M).unwrap();
        let field = [p.neutral[0] + 0.35, p.neutral[1]];
        let mut g = Guide {
            pair: [p.neutral[0] - p.neutral[1], 0.0, 0.0],
            q: [1.0, 0.0, 0.0],
            tone: 0.0,
        };
        assert_eq!(p.compatible_target(field, g), p.neutral);
        g.q = [0.0; 3];
        assert_eq!(p.compatible_target(field, g), field);
        g.q = [0.001, 0.0, 0.0];
        let weak = p.compatible_target(field, g);
        assert!(weak[0] > p.neutral[0] && weak[0] < field[0]);
    }
    #[test]
    fn neutral_move_retains_compatible_or_unsupported_changes() {
        let mut g = Guide {
            pair: [0.0; 3],
            q: [1.0, 0.0, 0.0],
            tone: 0.0,
        };
        assert_eq!(retained_neutral_move([0.04, 0.0], [0.041, 0.0], g), 1.0);
        assert_eq!(retained_neutral_move([0.04, 0.0], [0.01, 0.0], g), 1.0);
        // A move along the unobserved direction cannot be vetoed by B/M.
        assert_eq!(retained_neutral_move([0.04, 0.0], [1.04, 1.0], g), 1.0);
        g.q = [0.0; 3];
        assert_eq!(retained_neutral_move([0.04, 0.0], [-0.8, 0.5], g), 1.0);
    }
    #[test]
    fn neutral_move_limits_crossing_past_a_trustworthy_ratio() {
        let g = Guide {
            pair: [0.0; 3],
            q: [1.0, 0.0, 0.0],
            tone: 0.0,
        };
        let retained = retained_neutral_move([0.04, 0.0], [-0.17, 0.0], g);
        assert!((retained - (0.04 + 0.075) / 0.21).abs() < 1e-14);
        assert!((0.04 + retained * (-0.17 - 0.04) + 0.075).abs() < 1e-14);
        let weak = Guide {
            q: [0.01, 0.0, 0.0],
            ..g
        };
        assert_eq!(retained_neutral_move([0.04, 0.0], [-0.17, 0.0], weak), 1.0);
    }
    #[test]
    fn neutral_move_bound_holds_for_each_pair_and_is_maximal() {
        let ratios = |v: [f64; 2]| [v[0] - v[1], v[0], v[1]];
        for a in -4..=4 {
            for b in -4..=4 {
                for c in -4..=4 {
                    for q in [
                        [1.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [0.0, 0.0, 1.0],
                        [1.0, 0.3, 0.1],
                    ] {
                        let before = [a as f64 * 0.09, b as f64 * 0.08];
                        let proposed = [c as f64 * 0.12, -a as f64 * 0.07];
                        let g = Guide {
                            pair: [0.03, 0.1, 0.07],
                            q,
                            tone: 0.0,
                        };
                        let t = retained_neutral_move(before, proposed, g);
                        assert!((0.0..=1.0).contains(&t));
                        let allowed = |t: f64, tolerance: f64| {
                            let fit =
                                std::array::from_fn(|k| before[k] + t * (proposed[k] - before[k]));
                            (0..3).all(|k| {
                                q[k].sqrt() * (ratios(fit)[k] - g.pair[k]).abs()
                                    <= q[k].sqrt() * (ratios(before)[k] - g.pair[k]).abs()
                                        + PAIR_SCALE
                                        + tolerance
                            })
                        };
                        assert!(allowed(t, 1e-12));
                        if t < 1.0 - 1e-7 {
                            assert!(!allowed(t + 1e-7, 0.0));
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn balanced_pair_is_ambiguous_not_extra_evidence_for_white() {
        let p = ColorPrior::new(P, M).unwrap();
        let mut g = Guide {
            pair: [p.neutral[0] - p.neutral[1], p.neutral[0], p.neutral[1]],
            q: [1.0, 0.0, 0.0],
            tone: 0.0,
        };
        assert_eq!(neutral_weight(g), 0.0001);
        g.pair[0] += 0.2;
        assert_eq!(neutral_weight(g), 0.0001);
        g.q = [0.0; 3];
        assert!((neutral_weight(g) - 0.0401).abs() < 1e-15);
    }
    #[test]
    fn one_survivor_repaired_and_nonfinite_sites_cannot_supply_ratios() {
        for (m, mask, repaired) in [
            ([0.3, 0.6, 1.0], [255, 0, 0], false),
            ([0.3, 0.6, 1.0], [255, 255, 0], true),
            ([f64::NAN, 0.6, 1.0], [255, 255, 0], false),
        ] {
            assert_eq!(evidence(m, mask, repaired).1, [0.0; 3]);
        }
    }
    #[test]
    fn donorless_components_are_neutral_without_fabricating_hue() {
        let mut f = line(20);
        f.nodes[0].fixed = false;
        f.nodes[0].affected = true;
        f.reconstruct(Control::none()).unwrap();
        for n in &f.nodes {
            for ch in 0..2 {
                assert!((n.color[ch] - f.prior.neutral[ch]).abs() < 1e-10);
            }
        }
    }
    #[test]
    fn credible_pair_constrains_color_instead_of_forcing_neutral() {
        let mut f = line(20);
        f.nodes[0].fixed = false;
        f.nodes[0].affected = true;
        let p = f.prior.neutral;
        for n in &mut f.nodes {
            n.guide.q = [1.0, 0.0, 0.0];
            n.guide.pair[0] = p[0] - p[1] - 0.2;
        }
        f.reconstruct(Control::none()).unwrap();
        let c = f.nodes[10].color;
        assert!((c[0] - c[1] - (p[0] - p[1] - 0.2)).abs() < 0.001);
        assert!(c[0] < p[0] && c[1] < p[1]);
        assert!((dot3(Y, direction(c, Y).unwrap()) - 1.0).abs() < 1e-12);
    }
    #[test]
    fn uncertain_color_relaxes_smoothly_and_search_has_no_radius_cutoff() {
        let mut f = line(200);
        let p = f.prior;
        let color = [p.neutral[0] - 0.3, p.neutral[1] - 0.12];
        f.nodes[0].color = color;
        f.reconstruct(Control::none()).unwrap();
        let mut previous = f64::INFINITY;
        for n in &f.nodes {
            let a = n.color[0] - p.neutral[0];
            let b = n.color[1] - p.neutral[1];
            let energy = p.metric[0] * a * a + p.metric[1] * b * b + 2.0 * p.metric[2] * a * b;
            assert!(energy < previous + 1e-7);
            previous = energy;
        }
        assert_eq!(f.nodes[0].color, color);
        assert!(previous < 1e-10);
        assert!(f.nodes[199].distance > 1000.0 && f.nodes[199].distance.is_finite());
    }
    #[test]
    fn reference_gate_rejects_a_gentle_walk_to_a_dark_surface() {
        let a = Guide {
            tone: 0.0,
            ..Guide::default()
        };
        let b = Guide {
            tone: -3.0 * std::f64::consts::LN_2,
            ..a
        };
        assert!(surface_agreement(a, b) < 0.001);
        assert!(
            affinity(
                a,
                Guide {
                    tone: -0.2 * std::f64::consts::LN_2,
                    ..a
                }
            ) > 0.7
        );
    }
    #[test]
    fn unaffected_low_signal_cells_do_not_bridge_unrelated_donors() {
        let mut f = line(20);
        f.nodes[0].color[0] -= 0.3;
        for n in &mut f.nodes[1..10] {
            n.affected = false;
        }
        f.reconstruct(Control::none()).unwrap();
        assert!(!f.nodes[19].distance.is_finite());
        for ch in 0..2 {
            assert!((f.nodes[19].color[ch] - f.prior.neutral[ch]).abs() < 1e-10);
        }
    }
    #[test]
    fn native_repaired_targets_do_not_inject_color_and_keep_y() {
        let mut f = line(20);
        f.reconstruct(Control::none()).unwrap();
        let expected = direction(f.prior.neutral, Y).unwrap();
        let a = f
            .estimate(0, 80, [0.9, 0.3, 1.0], [255, 255, 0], true, 1.0, Y)
            .unwrap();
        for ch in 0..3 {
            assert!((a[ch] - expected[ch]).abs() < 1e-10);
        }
        assert!((dot3(Y, a) - 1.0).abs() < 1e-12);
        assert!(f
            .estimate(0, 80, [1.0; 3], [0; 3], false, f64::NAN, Y)
            .is_none());
    }
    #[test]
    fn invalid_metric_and_nonfinite_solve_fail() {
        assert!(ColorPrior::new(P, [0.0; 9]).is_err());
        let rows = [Equation {
            spatial_metric: [1.0, 1.0, 0.0],
            block: [f64::NAN, 1.0, 0.0],
            neighbors: [NONE; 4],
            weights: [0.0; 4],
        }];
        assert!(solve(&rows, &[[1.0; 2]], &mut [[0.0; 2]], Control::none()).is_err());
    }
    #[test]
    fn metric_operator_matches_dense_symmetric_positive_system() {
        let metric = [2.0, 4.0, 0.6];
        let rows = [
            Equation {
                spatial_metric: metric,
                block: [3.0, 5.0, 0.6],
                neighbors: [1, NONE, NONE, NONE],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            Equation {
                spatial_metric: metric,
                block: [3.0, 5.0, 0.6],
                neighbors: [0, NONE, NONE, NONE],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let dense = [
            [3.0, 0.6, -2.0, -0.6],
            [0.6, 5.0, -0.6, -4.0],
            [-2.0, -0.6, 3.0, 0.6],
            [-0.6, -4.0, 0.6, 5.0],
        ];
        let truth = [[0.3, -0.5], [0.9, 0.1]];
        let flat = [0.3, -0.5, 0.9, 0.1];
        let expected: [f64; 4] =
            std::array::from_fn(|i| (0..4).map(|j| dense[i][j] * flat[j]).sum());
        let mut rhs = [[0.0; 2]; 2];
        product(&rows, &truth, &mut rhs);
        for (actual, expected) in rhs.iter().flatten().zip(expected) {
            assert!((actual - expected).abs() < 1e-14);
        }
        assert!(dot(&truth, &rhs) > 0.0);
        let other = [[-0.1, 0.7], [0.4, -0.8]];
        let mut mapped = [[0.0; 2]; 2];
        product(&rows, &other, &mut mapped);
        assert!((dot(&truth, &mapped) - dot(&other, &rhs)).abs() < 1e-14);
        let mut result = [[0.0; 2]; 2];
        solve(&rows, &rhs, &mut result, Control::none()).unwrap();
        for (a, b) in result.iter().flatten().zip(truth.iter().flatten()) {
            assert!((a - b).abs() < 1e-9);
        }
    }
    #[test]
    fn neutral_boundary_and_ratio_use_consistent_missing_layer_direction() {
        let mut f = line(25);
        let p = f.prior;
        let delta = -0.29;
        for n in &mut f.nodes[1..] {
            n.guide.q = [1.0, 0.0, 0.0];
            n.guide.pair[0] = p.neutral[0] - p.neutral[1] + delta;
        }
        f.reconstruct(Control::none()).unwrap();
        let tangent = inverse(p.metric, [1.0, -1.0]);
        for n in &f.nodes[1..] {
            let z = [n.color[0] - p.neutral[0], n.color[1] - p.neutral[1]];
            // Both anchors and regularization are neutral. The response must lie
            // along C^-1*[1,-1], even next to the intact neutral boundary.
            assert!((z[0] * tangent[1] - z[1] * tangent[0]).abs() < 1e-7);
            assert!(z[0] < 0.0 && z[1] < 0.0);
        }
        assert_eq!(f.nodes[0].color, p.neutral);
    }
    #[test]
    fn cancellation_is_reported_not_silently_replaced() {
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let mut f = line(200);
        assert!(f.reconstruct(Control::new(&cancel)).is_err());
        let rows = [Equation {
            spatial_metric: [1.0, 1.0, 0.0],
            block: [1.0, 1.0, 0.0],
            neighbors: [NONE; 4],
            weights: [0.0; 4],
        }];
        assert!(solve(&rows, &[[1.0; 2]], &mut [[0.0; 2]], Control::new(&cancel)).is_err());
    }
}
