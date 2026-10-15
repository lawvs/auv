# Erratum: 2026-09-30 Trajectory-Geometry Spike (bcdcf867)

**Date:** 2026-10-15
**Amends:** `docs/ai/references/3dgs/2026-09-30-traj-geo-spike.md`
**Evidence:** Gate 0/1 re-evaluation, branch `spike/traj-geo-gate01` @ `64f48c67`,
report `docs/ai/references/3dgs/2026-10-15-traj-geo-gate01-retest.md`
(offline re-run on the same 75 captured frames; zero new captures,
zero production changes).

## Withdrawn

The verdict "trajectory-geometry route thoroughly Closed / impossible" is
**withdrawn**. Its quantitative basis was contaminated by four defects in the
original evaluator (verified line-by-line against `bcdcf867`):

1. All ground-truth structure AABBs were shifted by (−0.5, −0.5, −0.5) m: the
   evaluator sampled `[center−0.5, center+0.5]` while `/fill` blocks occupy
   `[b, b+1]`. Corrected per structure against the arena commands.
2. The completeness denominator included never-visible surfaces (ground behind
   the start position, Z > 12; structure top faces above eye level). Corrected
   to visible South faces only (frustum + facing rule).
3. The report claimed 132 GT sample points; the code generated 1,300+ (the 132
   figure was a stale draft carryover). Corrected count under the visibility
   definition: exactly 1,104 points at 0.5 m grid.
4. The depth arm did `z_cam = raw_d` — treating Depth Anything's
   disparity-like output as metric distance with zero calibration, despite a
   comment claiming camera-height calibration.

With the fixed evaluator (Gate 0 self-test: perfect synthetic cloud →
0.000 m / 100%), depth fusion with per-frame OLS disparity calibration
anchored to known camera height (1.62 m) and 3-frame voxel consensus
(7,585 voxels) passes all three frozen gates on the same 75 frames:

- median distance to visible GT surface: **0.375 m** (gate < 0.50 m) — PASS
- visible surface completeness: **78.8%** (gate > 60.0%) — PASS
- ground-plane Y residual std: **0.159 m** (gate < 0.20 m) — PASS

(Noted honestly: P90 tail 3.125 m — far-field voxels unfiltered; the frozen
gates did not constrain it.)

## Stands, now as a clean measurement

"3DGS as geometric substrate: NO-GO" **stands** — it was not an artifact of
the broken evaluator. Re-measured under the fixed evaluator, vanilla Brush
splats (7,146 opacity-filtered Gaussians, 1000 steps, no seed) give
**2.009 m / 4.4%** — essentially unchanged from the old report's 2.041 m /
4.1%. The failure is real: without an `init.ply` seed, Brush does not
converge to the surface on this forward-dominant trajectory.

## What this does not change

- This does **not** satisfy the 3DGS reopen condition. What passed is
  MVS-style depth fusion (borrowing telemetry eye height for calibration),
  not a splat substrate. The line remains at zero priority per the
  2026-10-03 freeze decision.
- Scope: 75 offline frames only. No new captures, no live runs.
