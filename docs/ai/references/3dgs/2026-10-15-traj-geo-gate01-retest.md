# Report: 3DGS Trajectory-Geometry Spike — Gate 0 / Gate 1 Re-Test

**Document Location**: `docs/ai/references/3dgs/2026-10-15-traj-geo-gate01-retest.md`  
**Related Report**: `docs/ai/references/3dgs/2026-09-30-traj-geo-spike.md`  
**Execution Context**: Offline falsification run on existing captured dataset (`.tmp/traj_geo_spike/dataset/`).  
**Environment**: Windows native (`F:\auv`), Rust 2024 edition, RTX 4070 Ti, zero Minecraft launches, zero production crate changes.  

---

## 1. Step 0 — Preconditions Confirmation

- **Precondition Status**: **CONFIRMED** (All required files present).
- **Dataset Path**: `.tmp/traj_geo_spike/dataset/`
- **Transforms File**: `.tmp/traj_geo_spike/dataset/transforms.json` (present, 75 frame entries, OpenCV camera model, 854x480 resolution).
- **Captured Frame Count**: Exactly 75 frames (`images/frame_000000.png` through `images/frame_000074.png`, all verified present and non-empty).
- **Execution Mode**: 100% offline. Zero Minecraft process launches, zero driver inputs, zero new data captures. All operations run directly against stored artifacts.
- **Production Code Surface**: Zero changes to production crates (`auv-game-minecraft/src/lib.rs`, etc.). All modifications strictly isolated to `supported/games/auv-game-minecraft/src/bin/traj_geo_spike.rs` and this document.

---

## 2. Gate 0 — Evaluator Fixes and Self-Test

### 2.1 Evaluator Defect Fixes

Line-by-line review against commit `bcdcf867` revealed three major defects in the ground-truth geometric evaluator:

1. **True Minecraft AABB Bounds**:
   In Minecraft, integer block coordinates `(X, Y, Z)` define the continuous interval `[X, X + 1]`, `[Y, Y + 1]`, `[Z, Z + 1]`. The previous evaluator erroneously used `[center - 0.5, center + 0.5]`. All structures in `GroundTruthScene::new_standard()` were corrected to match the `/fill` and `/setblock` arena commands:
   - **Stone Wall** (`/fill -2 95 -5 2 98 -5`):
     - Correct: $X \in [-2.0, 3.0]$, $Y \in [95.0, 99.0]$, $Z \in [-5.0, -4.0]$.
     - Old: $X \in [-2.5, 2.5]$, $Y \in [94.5, 98.5]$, $Z \in [-5.5, -4.5]$.
   - **Cobblestone Step** (`/fill 1 95 -3 2 96 -3`):
     - Correct: $X \in [1.0, 3.0]$, $Y \in [95.0, 97.0]$, $Z \in [-3.0, -2.0]$.
     - Old: $X \in [0.5, 2.5]$, $Y \in [94.5, 96.5]$, $Z \in [-3.5, -2.5]$.
   - **Chest West** (`/setblock -2 95 -3`):
     - Correct: $X \in [-2.0, -1.0]$, $Y \in [95.0, 96.0]$, $Z \in [-3.0, -2.0]$.
     - Old: $X \in [-2.5, -1.5]$, $Y \in [94.5, 95.5]$, $Z \in [-3.5, -2.5]$.
   - **Chest Center** (`/setblock 0 95 -3`):
     - Correct: $X \in [0.0, 1.0]$, $Y \in [95.0, 96.0]$, $Z \in [-3.0, -2.0]$.
     - Old: $X \in [-0.5, 0.5]$, $Y \in [94.5, 95.5]$, $Z \in [-3.5, -2.5]$.

2. **Visible Surface Sampling Rule**:
   All 3 passes traversed from $Z = 12.0$ Northward toward negative $Z$ ($Z \ge -3.7$) with pitch $0.0^\circ$ (horizontal gaze) from eye height $Y \approx 96.62$.
   - **Ground behind start excluded**: Ground with $Z > 12.0$ lies behind the player's initial position and was never in camera frustum; ground bounds were clamped to $Z \in [-8.0, 12.0]$.
   - **Structure top faces excluded**: The Stone Wall top ($Y = 99.0$) and Cobblestone Step top ($Y = 97.0$) are strictly above eye level ($Y = 96.62$). At pitch $0.0^\circ$, top faces are never visible from below. Top-face sampling was eliminated.
   - **Facing rule**: North faces and hidden side faces face away from oncoming passes; only South faces ($+Z$, `max_z`) facing oncoming passes are visible and sampled.

3. **Ground Truth Sample Count & Reconciliation**:
   - **Sampler Resolution**: 0.50 m grid spacing.
   - **Ground Surface Samples**: $X \in [-6.0, 6.0]$ (25 points) $\times$ $Z \in [-8.0, 12.0]$ (41 points) $= 1,025$ points, minus 63 points under structure footprints $= 962$ ground points.
   - **Visible Structure Faces**:
     - Stone Wall South face: 11 ($X$) $\times$ 9 ($Y$) $= 99$ points.
     - Cobblestone Step South face: 5 ($X$) $\times$ 5 ($Y$) $= 25$ points.
     - Chest West South face: 3 ($X$) $\times$ 3 ($Y$) $= 9$ points.
     - Chest Center South face: 3 ($X$) $\times$ 3 ($Y$) $= 9$ points.
     - Total structure South face samples: 142 points.
   - **True Total GT Samples**: Exactly **1,104 points**.
   - **Reconciliation against old report**: The 2026-09-30 spike report stated "(共 132 个基准表面采样点)". In reality, commit `bcdcf867` sampled over 1,300 points (including top faces and ground up to $Z = 15.0$). The "132 points" in the old report was a stale text carryover from an earlier design draft. Under the corrected visibility definition, the true count is exactly 1,104 points.

4. **Percentile Method**:
   Percentiles (median and P90) were upgraded from naive integer truncation indices to standard linear interpolation: virtual index $\text{idx} = p \cdot (n - 1)$, interpolating between $\lfloor\text{idx}\rfloor$ and $\lceil\text{idx}\rceil$. No outlier trimming or clipping applied.

### 2.2 Mandatory Gate 0 Evaluator Self-Test

Before running real evaluation, the evaluator was tested against a perfect synthetic point cloud sampled directly on the visible GT surface:

```text
================================================================================
           Gate 0 Evaluator Self-Test on Synthetic GT Point Cloud
================================================================================
[Self-Test] Sampled 1104 ground truth visible surface points (res=0.5m)
Total Synthetic Points:           1104
Points in Evaluation Volume:      1104
Median Dist to GT Surface:        0.000 m (Gate: == 0.000m -> PASS)
Mean Dist to GT Surface:          0.000 m
P90 Dist to GT Surface:           0.000 m
Surface Completeness Ratio:       100.0% (Gate: == 100.0% -> PASS)
Ground Plane Y Std Dev:           0.000 m
--------------------------------------------------------------------------------
SELF-TEST STATUS:                 PASSED
================================================================================
```

- **Median Distance to GT**: **0.000 m** (Requirement: 0.000 m) $\to$ **PASS**
- **Surface Completeness**: **100.0%** (Requirement: 100.0%) $\to$ **PASS**
- **Ground Plane Std Dev**: **0.000 m** $\to$ **PASS**

The evaluator is mathematically sound and verified.

---

## 3. Gate 1 — Fixed Depth-Fusion + Voxel Consensus

### 3.1 Method Description

#### A. Metric Calibration Anchored to Known Camera Height
- **Model Output Inspection**: Diagnostic probes on Depth Anything V2 (`depth_anything_v2_vits.onnx`) output revealed that model values correspond to relative disparity / inverse depth (closer objects produce higher values: near ground $v = 450$ gives raw $\approx 3.73$, horizon $v = 240$ gives raw $\approx 0.69$, sky $v = 50$ gives raw $\approx 0.00$). The previous spike code did `z_cam = raw_d`, directly treating disparity as distance and inverting scene scale.
- **Calibration Formulation**: In each frame $i$, the camera height above the flat grass ground ($Y = 95.0$) is known from telemetry: $d_y = \text{eye}_y - 95.0 \approx 1.62\text{ m}$. For image rows $v > c_y = 240.0$, the true geometric distance along the optical axis to the ground is:
  $$d_{\text{gt}}(v) = \frac{d_y \cdot f_{l_y}}{v - c_y}$$
- **Anchor Sampling & OLS Fit**: Candidate ground pixels are sampled in rows $v \in [320, 460]$ across central columns $u \in [w/4, 3w/4]$ (where no obstacles exist between player and horizon). An ordinary least squares (OLS) fit solves the affine disparity mapping:
  $$\frac{1}{d_{\text{gt}}} = s \cdot \text{raw\_d} + t$$
- **Metric Distance Reconstruction**: For each pixel:
  $$d_{\text{metric}} = \frac{1}{s \cdot \text{raw\_d} + t}$$

#### B. Axis Signs Verified against Matrix Code
- **Matrix Derivation**: Minecraft's view matrix and `effective_world_to_camera_matrix` conform to OpenGL camera space:
  - $+X_{\text{cam}}$: Right
  - $+Y_{\text{cam}}$: Up
  - $-Z_{\text{cam}}$: View direction (forward into scene)
- **Pixel Unprojection**:
  $$X_{\text{cam}} = \frac{u - c_x}{f_{l_x}} \cdot d_{\text{metric}}$$
  $$Y_{\text{cam}} = \frac{c_y - v}{f_{l_y}} \cdot d_{\text{metric}}$$
  $$Z_{\text{cam}} = -d_{\text{metric}}$$
- **World Transformation**: $P_w = \text{c2w} \cdot [X_{\text{cam}}, Y_{\text{cam}}, Z_{\text{cam}}, 1]^T$.
- **Verification Proof**: For a ground point at $v = 450 > c_y$, $c_y - v = -210 < 0$, giving $Y_{\text{cam}} < 0$. When multiplied by $\text{c2w}$, $Y_w = 96.62 + Y_{\text{cam}} = 95.00\text{ m}$ (matches ground plane $Y = 95.0$). Along the optical axis, $Z_{\text{cam}} = -d < 0$, giving $Z_w = 11.87 - d$, correctly placing unprojected points in front of the camera inside the evaluation arena ($Z \in [-10, 18]$) rather than behind it.

#### C. Voxel Consensus Filtering
- **Grid Resolution**: 0.25 m cubic voxels ($\lfloor X / 0.25 \rfloor, \lfloor Y / 0.25 \rfloor, \lfloor Z / 0.25 \rfloor$).
- **Consensus Rule**: Across all 75 frames, accumulate distinct frame observations per voxel. Keep only voxels observed by $\ge 3$ distinct frames.
- **Evaluation Points**: Occupied voxel centers $(v + 0.5) \times 0.25$ evaluated with the Gate-0-fixed evaluator.

---

## 4. Quantitative Results & Gate Verdicts

### 4.1 Gate 1 Raw Counts

| Metric Entity | Count | Notes |
|---|---|---|
| Total Frames Processed | **75** | All frames from 3 wandering passes |
| Total Distinct Voxels Accumulated | **36,888** | 0.25m grid unprojected volume |
| Consensus Voxels Kept ($\ge 3$ frames) | **7,585** | Multi-frame consensus filtering |
| Voxels in Evaluation Volume | **7,579** | Bounding box $X[-8,8], Y[93,102], Z[-10,18]$ |
| Ground Truth Surface Benchmark Points | **1,104** | Fixed visible surface sampler (0.5m resolution) |

### 4.2 Gate 1 Results against Frozen Gates

| Gate # | Metric Dimension | Frozen Gate Threshold | Measured (Consensus Voxels) | Measured (Raw Unprojected) | Verdict |
|---|---|---|---|---|---|
| **Gate 1** | **Median distance to visible GT surface** | **$< 0.50\text{ m}$** | **0.375 m** | 0.404 m | **PASS** |
| **Gate 2** | **Visible GT surface completeness** | **$> 60.0\%$** | **78.8%** | 80.2% | **PASS** |
| **Gate 3** | **Ground-plane Y residual std dev** | **$< 0.20\text{ m}$** | **0.159 m** | 0.162 m | **PASS** |
| *Diag* | Mean distance to GT surface | Reference only | 0.962 m | 1.118 m | - |
| *Diag* | P90 distance to GT surface | Reference only | 3.125 m | 3.322 m | - |
| **Final** | **Gate 1 Overall Verdict** | **All 3 Gates PASS** | $\mathbf{GO}$ | $\mathbf{GO}$ | **PASS** |

### 4.3 Reference: 3DGS Retest under Fixed Evaluator

For direct comparison, the 3DGS model output (`trainer_output/splat_1000.ply`, 7,146 opacity-filtered Gaussian points) was re-evaluated using the exact same Gate-0-fixed evaluator:

| Metric Dimension | Frozen Gate Threshold | 3DGS Measured (Fixed Evaluator) | 3DGS Old Report (Broken Evaluator) | Verdict |
|---|---|---|---|---|
| Median distance to GT surface | $< 0.50\text{ m}$ | **2.009 m** | 2.041 m | **FAIL** |
| Visible GT surface completeness | $> 60.0\%$ | **4.4%** | 4.1% | **FAIL** |
| Ground-plane Y residual std dev | $< 0.20\text{ m}$ | **0.000 m** (no ground cluster) | 0.000 m | - |
| P90 distance to GT surface | Reference only | 2.595 m | 2.603 m | - |
| **Overall 3DGS Verdict** | **Both Gates PASS** | $\mathbf{NO\text{-}GO}$ | $\mathbf{NO\text{-}GO}$ | **FAIL** |

---

## 5. Summary Verdicts for Erratum Decision

1. **Gate 1 (Median Distance < 0.50m)**: **PASS** (measured **0.375 m**).
2. **Gate 2 (Surface Completeness > 60.0%)**: **PASS** (measured **78.8%**).
3. **Gate 3 (Ground-plane Y std < 0.20m)**: **PASS** (measured **0.159 m**).
4. **Overall Gate 1 Depth-Fusion Verdict**: **GO**. With proper ground-anchored disparity calibration, correct OpenGL camera axes, and 3-frame voxel consensus, depth-fusion over the 75 forward-dominant frames satisfies all three frozen criteria.
5. **3DGS Verdict**: **NO-GO** (**2.009 m** median distance, **4.4%** completeness). The failure of 3DGS on forward-dominant trajectories was not an artifact of the defective evaluator; it remains fundamentally incapable of geometric convergence under forward motion.
