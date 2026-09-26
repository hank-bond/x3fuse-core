# Accepted G1 recovery checkpoint

Highlight-recovery research is finished for now by user decision following review
of nine DP2 Merrill scenes, especially the red flower: “the red flower really sells
it”. This is the accepted **G1 tone + C2 color** checkpoint, not a release or an app
installation. G1 remains explicitly opt-in. No LUT/profile fitting is included.

## Frozen reconstruction

- T1 supplies a soft tonal anchor, not a brightness curve fitted to SPP.
- G1 solves positive log pre-onset target luminance at native resolution. Its
  four-neighbor objective is `1/64 * sum((x-x_T1)^2)` plus reliable individual-layer
  log-gradient errors, with absolute weights `min(r_i,c, r_j,c)^2`.
- Only finite, positive layer pairs supply gradients. Healthy/ineligible pixels
  remain fixed; targets with no usable gradient support retain T1 exactly.
- Jacobi-preconditioned conjugate gradients allow at most 1,000 iterations and
  require true infinity residual `<= 1e-9 * max(1, |rhs|_infinity)`. Invalid outputs
  or convergence failure stop the run; they do not trigger automatic retuning.
- An immutable field is built before both headroom and encoding passes. The
  original recovery onset is applied once, followed by unchanged C2 colorization.
- No feathering, coarse-grid interpolation, sharpening, baked SPP shoulder, or
  denoise/repair/shading change was added to G1.

The implementation and its supporting recovery/diagnostic modules are retained
byte-for-byte from the reviewed source. Earlier experimental selectors remain
research controls, not additional accepted candidates or a production API.

## Reproducing the accepted configuration

Use native sensor recovery with `-dng-highlight-recovery`, denoise 10, normal WB
radial/CAMF spatial correction, **no external OpcodeList3**, and linear headroom
encoding (no shoulder override). Clear inherited `X3F_*` overrides before setting:

| Selector | Accepted value |
|---|---|
| `X3F_EXPERIMENT_GRADIENT_TONE` | `g1` |
| `X3F_EXPERIMENT_SURVIVOR_TONE` | Explicit per-image linear encoding floor, 1..16 |
| `X3F_EXPERIMENT_TONE_BOUNDARY` | `hold` |
| `X3F_EXPERIMENT_TONE_LUMA` | Three camera-PCS Y coefficients derived from that image's DNG metadata |
| `X3F_EXPERIMENT_TONE_COLOR_RADIUS` | `256` (C2) |
| `X3F_EXPERIMENT_TONE_SEVERE` | `guarded` |

Do not copy another image's luminance coefficients or assume its encoding floor is
universal. Natural maxima can raise the floor; comparison controls must share the
final linear scale/BaselineExposure. Existing experiment controllers record the
exact per-image commands. The gradient selector alone is not sufficient. This
checkpoint does not establish G1 support for Quattro or other camera families.

## Evidence and limits

The companion `spp-profile` workspace (not this Git repository) preserves:

- `reports/recovery-gradient-tone-01/`: shirt pilot and source snapshots.
- `reports/recovery-gradient-generalization-01/`: cloud and pink petals.
- `reports/recovery-gradient-broad-01/`: six additional SPP/G1 comparisons.
- `reports/recovery-gradient-closeout-01/`: subsequent acceptance and commit receipt.
- Matching `training/runs/` manifests, commands, DNGs and SDK decodes/renders.

Frozen reviewed binary, relative to this repository:
`target-gradient-tone-01/aarch64-apple-darwin/release/x3f_extract`.
SHA-256: `eda9bcf6ecaf6bf42267bb99476fd5b4dd2d19228192c4d041a44021d244f10c`.
Build directories and image artifacts are not committed; do not rebuild over this
archived binary. Use a fresh target directory for checks or future changes.

The six-scene batch verified 87,449,768 healthy native pixels exactly against
matched T1, 53,248 trace encodings, and 152 gallery panels independently recreated
from TIFFs. Unit tests cover solver/evidence/onset behavior. These checks support
invariants, not physically correct reconstruction of every missing detail.

G1 can shift broad tone or misinterpret material-color gradients as luminance.
Bright pink-petal detail can darken; the shirt's entire hand crop is not protected,
although genuinely healthy pixels remain exact. Yellow/fluorescent scenes barely
activate recovery and are preservation checks, not broad G1 improvement evidence.
The old ignored cloud-onset characterization and red-speck/repair issues remain
separate, deferred work. Visual acceptance does not erase these limitations.

## Next phase: healthy-region LUT

Keep recovery frozen. The user wants a new LUT profiled/aligned to SPP from regions
outside highlight recovery. Before implementing, agree the source-derived exclusion
mask and its transformation into aligned training coordinates. Excluding only
G1-versus-T1 changed pixels would miss other recovery operations; display-white
thresholding is not a substitute for recovery evidence. Boundary margins and
resampling support need an explicit policy.

Fit and validate on the retained regions without teaching the LUT to undo recovery.
Continue checking recovered highlights in full-image renders: excluding them from
training does not prevent a global LUT from changing them at application time.
Mask export, alignment changes, LUT training, production promotion and installation
are not part of this checkpoint.
