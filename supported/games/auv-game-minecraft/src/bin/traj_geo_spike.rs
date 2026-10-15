//! Traj-Geo-Spike: Trajectory Accumulation Background 3D Geometry Spike
//!
//! Purpose:
//!   Verify whether normal gameplay single-view trajectories (forward-dominant,
//!   no 180-deg turning, slight intentional head-bob/saccades, >= 3 passes wandering)
//!   accumulated over an area can produce geometrically usable scene silhouettes
//!   via the legacy 3DGS pipeline (Brush 0.3.0) or depth-fusion baseline.
//!
//! Non-Goals:
//!   - No monocular VO / scale ambiguity (telemetry used directly as GT answer key).
//!   - Zero touch to production Rust code (lives entirely in this spike binary).
//!   - Not pursuing photorealistic visual novel-view synthesis (already disproven).
//!
//! Frozen Geometric Criteria (frozen before evaluation):
//!   - Median distance from reconstructed points to GT surface < 0.5m
//!   - GT surface completeness (covered ratio by reconstructed points) > 60%

use auv_game_minecraft::ingest::read_latest_spatial_frame_from_tail;
use auv_game_minecraft::types::{MinecraftSpatialFrame, Viewport};
use auv_game_minecraft::visual_perception::DepthEstimator;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// -----------------------------------------------------------------------------
// Win32 Input & Window Helpers
// -----------------------------------------------------------------------------

mod win32_input {
  use super::*;

  #[repr(C)]
  #[derive(Clone, Copy)]
  pub struct MOUSEINPUT {
    pub dx: i32,
    pub dy: i32,
    pub mouse_data: u32,
    pub dw_flags: u32,
    pub time: u32,
    pub dw_extra_info: usize,
  }

  #[repr(C)]
  #[derive(Clone, Copy)]
  pub struct KEYBDINPUT {
    pub w_vk: u16,
    pub w_scan: u16,
    pub dw_flags: u32,
    pub time: u32,
    pub dw_extra_info: usize,
  }

  #[repr(C)]
  #[derive(Clone, Copy)]
  pub struct HARDWAREINPUT {
    pub u_msg: u32,
    pub w_param_l: u16,
    pub w_param_h: u16,
  }

  #[repr(C)]
  #[derive(Clone, Copy)]
  pub union INPUT_UNION {
    pub mi: MOUSEINPUT,
    pub ki: KEYBDINPUT,
    pub hi: HARDWAREINPUT,
  }

  #[repr(C)]
  #[derive(Clone, Copy)]
  pub struct INPUT {
    pub r#type: u32,
    pub u: INPUT_UNION,
  }

  pub const INPUT_MOUSE: u32 = 0;
  pub const INPUT_KEYBOARD: u32 = 1;
  pub const MOUSEEVENTF_MOVE: u32 = 0x0001;
  pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
  pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
  pub const KEYEVENTF_KEYUP: u32 = 0x0002;
  pub const KEYEVENTF_SCANCODE: u32 = 0x0008;

  pub const SW_RESTORE: i32 = 9;
  pub const SCANCODE_W: u16 = 0x11;
  pub const VK_W: u16 = 0x57;
  pub const VK_SLASH: u16 = 0xBF;
  pub const VK_RETURN: u16 = 0x0D;

  pub type HWND = *mut c_void;

  #[link(name = "user32")]
  unsafe extern "system" {
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> i32;
    fn BringWindowToTop(hWnd: HWND) -> i32;
    fn SetForegroundWindow(hWnd: HWND) -> i32;
    fn GetForegroundWindow() -> HWND;
    fn AttachThreadInput(idAttach: u32, idAttachTo: u32, fAttach: i32) -> i32;
    fn GetWindowThreadProcessId(hWnd: HWND, lpdwProcessId: *mut u32) -> u32;
    fn SendInput(cInputs: u32, pInputs: *const INPUT, cbSize: i32) -> u32;
    fn PostMessageW(hWnd: HWND, Msg: u32, wParam: usize, lParam: isize) -> i32;
    fn OpenDesktopA(lpszDesktop: *const u8, dwFlags: u32, fInherit: i32, dwDesiredAccess: u32) -> *mut c_void;
    fn SetThreadDesktop(hDesktop: *mut c_void) -> i32;
  }

  #[link(name = "kernel32")]
  unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
  }

  pub static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

  pub fn ensure_default_desktop() {
    unsafe {
      let hdesk = OpenDesktopA(c"default".as_ptr().cast(), 0, 0, 0x01FF);
      if !hdesk.is_null() {
        let _ = SetThreadDesktop(hdesk);
      }
    }
  }

  pub fn activate_minecraft(target_hwnd: Option<isize>) -> bool {
    ensure_default_desktop();
    let hwnd = match target_hwnd {
      Some(h) if h != 0 => {
        TARGET_HWND.store(h, Ordering::SeqCst);
        h as HWND
      }
      _ => {
        let stored = TARGET_HWND.load(Ordering::SeqCst);
        if stored != 0 {
          stored as HWND
        } else {
          return false;
        }
      }
    };

    unsafe {
      let cur_fg = GetForegroundWindow();
      if cur_fg == hwnd {
        return true;
      }
      ShowWindow(hwnd, SW_RESTORE);
      sleep(Duration::from_millis(50));

      let cur_thread = GetCurrentThreadId();
      let mut target_pid: u32 = 0;
      let target_thread = GetWindowThreadProcessId(hwnd, &mut target_pid);

      if cur_thread != target_thread {
        AttachThreadInput(cur_thread, target_thread, 1);
      }

      BringWindowToTop(hwnd);
      SetForegroundWindow(hwnd);

      if cur_thread != target_thread {
        AttachThreadInput(cur_thread, target_thread, 0);
      }

      sleep(Duration::from_millis(100));

      // Left-click inside the window to lock mouse cursor in GLFW
      let click_down = INPUT {
        r#type: INPUT_MOUSE,
        u: INPUT_UNION {
          mi: MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouse_data: 0,
            dw_flags: MOUSEEVENTF_LEFTDOWN,
            time: 0,
            dw_extra_info: 0,
          },
        },
      };
      let click_up = INPUT {
        r#type: INPUT_MOUSE,
        u: INPUT_UNION {
          mi: MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouse_data: 0,
            dw_flags: MOUSEEVENTF_LEFTUP,
            time: 0,
            dw_extra_info: 0,
          },
        },
      };
      SendInput(1, &click_down, size_of::<INPUT>() as i32);
      sleep(Duration::from_millis(30));
      SendInput(1, &click_up, size_of::<INPUT>() as i32);
      sleep(Duration::from_millis(80));

      GetForegroundWindow() == hwnd
    }
  }

  pub fn turn_mouse(dx: i32, dy: i32) {
    ensure_default_desktop();
    let hwnd_val = TARGET_HWND.load(Ordering::SeqCst);
    if hwnd_val != 0 {
      let hwnd = hwnd_val as HWND;
      let cur = unsafe { GetForegroundWindow() };
      if cur != hwnd {
        activate_minecraft(Some(hwnd_val));
      }
    }
    let input = INPUT {
      r#type: INPUT_MOUSE,
      u: INPUT_UNION {
        mi: MOUSEINPUT {
          dx,
          dy,
          mouse_data: 0,
          dw_flags: MOUSEEVENTF_MOVE,
          time: 0,
          dw_extra_info: 0,
        },
      },
    };
    unsafe {
      SendInput(1, &input, size_of::<INPUT>() as i32);
    }
  }

  pub fn step_forward(duration: Duration) {
    ensure_default_desktop();
    let hwnd_val = TARGET_HWND.load(Ordering::SeqCst);
    if hwnd_val != 0 {
      let hwnd = hwnd_val as HWND;
      let cur = unsafe { GetForegroundWindow() };
      if cur != hwnd {
        activate_minecraft(Some(hwnd_val));
      }
    }
    let down = INPUT {
      r#type: INPUT_KEYBOARD,
      u: INPUT_UNION {
        ki: KEYBDINPUT {
          w_vk: VK_W,
          w_scan: 0x11,
          dw_flags: 0,
          time: 0,
          dw_extra_info: 0,
        },
      },
    };
    let up = INPUT {
      r#type: INPUT_KEYBOARD,
      u: INPUT_UNION {
        ki: KEYBDINPUT {
          w_vk: VK_W,
          w_scan: 0x11,
          dw_flags: KEYEVENTF_KEYUP,
          time: 0,
          dw_extra_info: 0,
        },
      },
    };
    unsafe {
      SendInput(1, &down, size_of::<INPUT>() as i32);
      sleep(duration);
      SendInput(1, &up, size_of::<INPUT>() as i32);
    }
  }

  pub fn send_chat_command(cmd: &str) {
    let hwnd_val = TARGET_HWND.load(Ordering::SeqCst);
    if hwnd_val == 0 {
      eprintln!("[Harness] ERROR: TARGET_HWND is 0, cannot send command: {cmd}");
      return;
    }
    let hwnd = hwnd_val as HWND;

    const WM_CHAR: u32 = 0x0102;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;

    let full_cmd = if cmd.starts_with('/') {
      cmd.to_string()
    } else {
      format!("/{cmd}")
    };

    println!("[Command] {full_cmd}");

    unsafe {
      PostMessageW(hwnd, WM_KEYDOWN, VK_SLASH as usize, 1 | (0x35 << 16));
      sleep(Duration::from_millis(30));
      PostMessageW(hwnd, WM_KEYUP, VK_SLASH as usize, 1 | (0x35 << 16) | (1 << 30) | (1 << 31));
      sleep(Duration::from_millis(250));

      for ch in full_cmd[1..].chars() {
        PostMessageW(hwnd, WM_CHAR, ch as usize, 1);
        sleep(Duration::from_millis(15));
      }
      sleep(Duration::from_millis(150));

      PostMessageW(hwnd, WM_KEYDOWN, VK_RETURN as usize, 1 | (0x1C << 16));
      sleep(Duration::from_millis(50));
      PostMessageW(hwnd, WM_KEYUP, VK_RETURN as usize, 1 | (0x1C << 16) | (1 << 30) | (1 << 31));
      sleep(Duration::from_millis(400));
    }
  }
}

// -----------------------------------------------------------------------------
// Matrix & Nerfstudio Dataset Math
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NerfstudioTransforms {
  pub camera_model: String,
  pub w: u32,
  pub h: u32,
  pub fl_x: f64,
  pub fl_y: f64,
  pub cx: f64,
  pub cy: f64,
  pub k1: f64,
  pub k2: f64,
  pub p1: f64,
  pub p2: f64,
  pub frames: Vec<NerfstudioFrame>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NerfstudioFrame {
  pub file_path: String,
  pub transform_matrix: [[f64; 4]; 4],
}

pub fn effective_world_to_camera_matrix(spatial_frame: &MinecraftSpatialFrame) -> Option<[f64; 16]> {
  let mut matrix = spatial_frame.view_matrix;
  const EPSILON: f64 = 1e-6;
  let is_rotation_only = matrix[12].abs() <= EPSILON
    && matrix[13].abs() <= EPSILON
    && matrix[14].abs() <= EPSILON
    && (spatial_frame.player_pose.eye_position.x.abs() > EPSILON
      || spatial_frame.player_pose.eye_position.y.abs() > EPSILON
      || spatial_frame.player_pose.eye_position.z.abs() > EPSILON);

  if is_rotation_only {
    let eye = spatial_frame.player_pose.eye_position;
    let translated = [
      matrix[0] * eye.x + matrix[4] * eye.y + matrix[8] * eye.z,
      matrix[1] * eye.x + matrix[5] * eye.y + matrix[9] * eye.z,
      matrix[2] * eye.x + matrix[6] * eye.y + matrix[10] * eye.z,
    ];
    matrix[12] = -translated[0];
    matrix[13] = -translated[1];
    matrix[14] = -translated[2];
  }

  let is_affine =
    matrix[3].abs() <= EPSILON && matrix[7].abs() <= EPSILON && matrix[11].abs() <= EPSILON && (matrix[15] - 1.0).abs() <= EPSILON;

  is_affine.then_some(matrix)
}

pub fn invert_affine_matrix(matrix: &[f64; 16]) -> Option<[f64; 16]> {
  let a00 = matrix[0];
  let a01 = matrix[4];
  let a02 = matrix[8];
  let a10 = matrix[1];
  let a11 = matrix[5];
  let a12 = matrix[9];
  let a20 = matrix[2];
  let a21 = matrix[6];
  let a22 = matrix[10];

  let c00 = a11 * a22 - a12 * a21;
  let c01 = -(a10 * a22 - a12 * a20);
  let c02 = a10 * a21 - a11 * a20;
  let c10 = -(a01 * a22 - a02 * a21);
  let c11 = a00 * a22 - a02 * a20;
  let c12 = -(a00 * a21 - a01 * a20);
  let c20 = a01 * a12 - a02 * a11;
  let c21 = -(a00 * a12 - a02 * a10);
  let c22 = a00 * a11 - a01 * a10;

  let det = a00 * c00 + a01 * c01 + a02 * c02;
  if !det.is_finite() || det.abs() <= 1e-9 {
    return None;
  }

  let inv_det = 1.0 / det;
  let inv3 = [
    c00 * inv_det,
    c01 * inv_det,
    c02 * inv_det,
    c10 * inv_det,
    c11 * inv_det,
    c12 * inv_det,
    c20 * inv_det,
    c21 * inv_det,
    c22 * inv_det,
  ];

  let tx = matrix[12];
  let ty = matrix[13];
  let tz = matrix[14];
  let itx = -(inv3[0] * tx + inv3[3] * ty + inv3[6] * tz);
  let ity = -(inv3[1] * tx + inv3[4] * ty + inv3[7] * tz);
  let itz = -(inv3[2] * tx + inv3[5] * ty + inv3[8] * tz);

  Some([
    inv3[0], inv3[1], inv3[2], 0.0, inv3[3], inv3[4], inv3[5], 0.0, inv3[6], inv3[7], inv3[8], 0.0, itx, ity, itz, 1.0,
  ])
}

pub fn intrinsics_from_projection(viewport: Viewport, projection: &[f64; 16]) -> Option<(f64, f64, f64, f64)> {
  let w = viewport.width as f64;
  let h = viewport.height as f64;
  if w <= 0.0 || h <= 0.0 {
    return None;
  }
  let fl_x = projection[0] * w / 2.0;
  let fl_y = projection[5] * h / 2.0;
  let cx = w / 2.0;
  let cy = h / 2.0;
  (fl_x.is_finite() && fl_y.is_finite() && fl_x > 0.0 && fl_y > 0.0).then_some((fl_x, fl_y, cx, cy))
}

pub fn matrix_rows(matrix: &[f64; 16]) -> [[f64; 4]; 4] {
  [
    [matrix[0], matrix[4], matrix[8], matrix[12]],
    [matrix[1], matrix[5], matrix[9], matrix[13]],
    [matrix[2], matrix[6], matrix[10], matrix[14]],
    [matrix[3], matrix[7], matrix[11], matrix[15]],
  ]
}

// -----------------------------------------------------------------------------
// Ground Truth Geometric Bounds & Evaluation
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AABB {
  pub name: String,
  pub min_x: f64,
  pub max_x: f64,
  pub min_y: f64,
  pub max_y: f64,
  pub min_z: f64,
  pub max_z: f64,
}

impl AABB {
  pub fn distance_to_point(&self, p: [f64; 3]) -> f64 {
    // Distance from point to box surface
    let dx = if p[0] < self.min_x {
      self.min_x - p[0]
    } else if p[0] > self.max_x {
      p[0] - self.max_x
    } else {
      0.0
    };
    let dy = if p[1] < self.min_y {
      self.min_y - p[1]
    } else if p[1] > self.max_y {
      p[1] - self.max_y
    } else {
      0.0
    };
    let dz = if p[2] < self.min_z {
      self.min_z - p[2]
    } else if p[2] > self.max_z {
      p[2] - self.max_z
    } else {
      0.0
    };

    if dx == 0.0 && dy == 0.0 && dz == 0.0 {
      // Inside box: distance to closest face
      let d_left = (p[0] - self.min_x).abs();
      let d_right = (self.max_x - p[0]).abs();
      let d_bottom = (p[1] - self.min_y).abs();
      let d_top = (self.max_y - p[1]).abs();
      let d_front = (p[2] - self.min_z).abs();
      let d_back = (self.max_z - p[2]).abs();
      d_left.min(d_right).min(d_bottom).min(d_top).min(d_front).min(d_back)
    } else {
      (dx * dx + dy * dy + dz * dz).sqrt()
    }
  }
}

pub struct GroundTruthScene {
  pub ground_y: f64,
  pub ground_bounds_x: (f64, f64),
  pub ground_bounds_z: (f64, f64),
  pub structures: Vec<AABB>,
}

impl GroundTruthScene {
  pub fn new_standard() -> Self {
    Self {
      ground_y: 95.0, // Top of grass block where feet stand
      ground_bounds_x: (-6.0, 6.0),
      ground_bounds_z: (-8.0, 12.0),
      structures: vec![
        // Stone Wall: /fill -2 95 -5 2 98 -5 (X from -2.0 to 3.0, Y from 95.0 to 99.0, Z from -5.0 to -4.0)
        AABB {
          name: "stone_wall".to_string(),
          min_x: -2.0,
          max_x: 3.0,
          min_y: 95.0,
          max_y: 99.0,
          min_z: -5.0,
          max_z: -4.0,
        },
        // Cobblestone Step: /fill 1 95 -3 2 96 -3 (X from 1.0 to 3.0, Y from 95.0 to 97.0, Z from -3.0 to -2.0)
        AABB {
          name: "cobblestone_step".to_string(),
          min_x: 1.0,
          max_x: 3.0,
          min_y: 95.0,
          max_y: 97.0,
          min_z: -3.0,
          max_z: -2.0,
        },
        // Chest 1 at (-2, 95, -3): /setblock -2 95 -3 (X from -2.0 to -1.0, Y from 95.0 to 96.0, Z from -3.0 to -2.0)
        AABB {
          name: "chest_west".to_string(),
          min_x: -2.0,
          max_x: -1.0,
          min_y: 95.0,
          max_y: 96.0,
          min_z: -3.0,
          max_z: -2.0,
        },
        // Chest 2 at (0, 95, -3): /setblock 0 95 -3 (X from 0.0 to 1.0, Y from 95.0 to 96.0, Z from -3.0 to -2.0)
        AABB {
          name: "chest_center".to_string(),
          min_x: 0.0,
          max_x: 1.0,
          min_y: 95.0,
          max_y: 96.0,
          min_z: -3.0,
          max_z: -2.0,
        },
      ],
    }
  }

  pub fn distance_to_scene(&self, p: [f64; 3]) -> f64 {
    // Distance to ground plane patch
    let dx = if p[0] < self.ground_bounds_x.0 {
      self.ground_bounds_x.0 - p[0]
    } else if p[0] > self.ground_bounds_x.1 {
      p[0] - self.ground_bounds_x.1
    } else {
      0.0
    };
    let dz = if p[2] < self.ground_bounds_z.0 {
      self.ground_bounds_z.0 - p[2]
    } else if p[2] > self.ground_bounds_z.1 {
      p[2] - self.ground_bounds_z.1
    } else {
      0.0
    };
    let dy = p[1] - self.ground_y;
    let d_ground = (dx * dx + dy * dy + dz * dz).sqrt();

    let mut min_d = d_ground;
    for s in &self.structures {
      let d = s.distance_to_point(p);
      if d < min_d {
        min_d = d;
      }
    }
    min_d
  }

  /// Generate sample points on the GT surfaces to evaluate coverage / completeness.
  /// Visibility rule: Poses advance North (yaw ~180°, pitch 0.0) from Z=12.0 to Z >= -3.7 from eye height 96.62.
  /// - Ground behind the start position (Z > 12.0) is excluded.
  /// - Structure top faces are excluded (wall Y=99 and step Y=97 are above eye height; cannot be seen from below at pitch 0).
  /// - Structure South faces (+Z, max_z) face the oncoming camera and are visible.
  pub fn sample_gt_surface_points(&self, resolution: f64) -> Vec<[f64; 3]> {
    let mut pts = Vec::new();
    // 1. Ground plane (visible region: Z in [-8.0, 12.0])
    let mut z = self.ground_bounds_z.0;
    while z <= self.ground_bounds_z.1 + 1e-6 {
      let mut x = self.ground_bounds_x.0;
      while x <= self.ground_bounds_x.1 + 1e-6 {
        // Skip under structures
        let mut under = false;
        for s in &self.structures {
          if x >= s.min_x - 1e-6 && x <= s.max_x + 1e-6 && z >= s.min_z - 1e-6 && z <= s.max_z + 1e-6 {
            under = true;
            break;
          }
        }
        if !under {
          pts.push([x, self.ground_y, z]);
        }
        x += resolution;
      }
      z += resolution;
    }

    // 2. Structure visible faces: South faces (+Z)
    for s in &self.structures {
      let mut x = s.min_x;
      while x <= s.max_x + 1e-6 {
        let mut y = s.min_y;
        while y <= s.max_y + 1e-6 {
          pts.push([x, y, s.max_z]);
          y += resolution;
        }
        x += resolution;
      }
    }
    pts
  }
}

// -----------------------------------------------------------------------------
// PLY Point Cloud Parser
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GaussianPoint {
  pub x: f64,
  pub y: f64,
  pub z: f64,
  pub opacity: f64,
}

pub fn parse_brush_ply(ply_path: &Path) -> Result<Vec<GaussianPoint>, String> {
  let file = File::open(ply_path).map_err(|e| format!("Failed to open PLY {}: {e}", ply_path.display()))?;
  let mut reader = BufReader::new(file);

  let mut vertex_count: usize = 0;
  let mut in_header = true;

  loop {
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(|e| format!("Error reading PLY header: {e}"))?;
    if n == 0 {
      break;
    }
    let trimmed = line.trim();
    if trimmed.starts_with("element vertex ") {
      let parts: Vec<&str> = trimmed.split_whitespace().collect();
      if parts.len() >= 3 {
        vertex_count = parts[2].parse().unwrap_or(0);
      }
    } else if trimmed == "end_header" {
      in_header = false;
      break;
    }
  }

  if in_header || vertex_count == 0 {
    return Err(format!("Invalid PLY header or 0 vertices in {}", ply_path.display()));
  }

  // Brush format binary_little_endian 1.0 has 59 float properties per vertex:
  // f_dc (3), f_rest (45), opacity (1), rot (4), scale (3), x (1), y (1), z (1) = 59 floats = 236 bytes.
  let vertex_stride = 59 * 4;
  let mut points = Vec::with_capacity(vertex_count);
  let mut buf = vec![0u8; vertex_stride];

  for _ in 0..vertex_count {
    reader.read_exact(&mut buf).map_err(|e| format!("Error reading vertex bytes: {e}"))?;
    let mut floats = [0.0f32; 59];
    for i in 0..59 {
      let b = &buf[i * 4..(i + 1) * 4];
      floats[i] = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    let logit_opacity = floats[48] as f64;
    let alpha = 1.0 / (1.0 + (-logit_opacity).exp());
    let x = floats[56] as f64;
    let y = floats[57] as f64;
    let z = floats[58] as f64;

    points.push(GaussianPoint {
      x,
      y,
      z,
      opacity: alpha,
    });
  }

  Ok(points)
}

// -----------------------------------------------------------------------------
// Geometric Quality Metrics Output
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeometricMetrics {
  pub total_points: usize,
  pub filtered_points: usize,
  pub filter_opacity_threshold: f64,
  pub points_in_eval_volume: usize,
  pub median_distance_to_gt_m: f64,
  pub mean_distance_to_gt_m: f64,
  pub p90_distance_to_gt_m: f64,
  pub surface_completeness_ratio: f64,
  pub ground_plane_y_std_m: f64,
  pub passes_median_gate: bool,
  pub passes_completeness_gate: bool,
  pub passes_ground_gate: bool,
  pub final_verdict: String,
}

pub fn percentile_linear(sorted: &[f64], p: f64) -> f64 {
  if sorted.is_empty() {
    return f64::MAX;
  }
  if sorted.len() == 1 {
    return sorted[0];
  }
  let p_clamped = p.clamp(0.0, 1.0);
  let idx = p_clamped * (sorted.len() - 1) as f64;
  let lower = idx.floor() as usize;
  let frac = idx - lower as f64;
  if lower + 1 < sorted.len() {
    sorted[lower] + frac * (sorted[lower + 1] - sorted[lower])
  } else {
    sorted[lower]
  }
}

pub fn evaluate_point_cloud(points: &[GaussianPoint], opacity_thresh: f64, scene: &GroundTruthScene) -> GeometricMetrics {
  let filtered: Vec<&GaussianPoint> =
    points.iter().filter(|p| p.opacity >= opacity_thresh && p.x.is_finite() && p.y.is_finite() && p.z.is_finite()).collect();

  // Bounding volume for evaluation: X in [-8, 8], Y in [93, 102], Z in [-10, 18]
  let in_volume: Vec<&GaussianPoint> =
    filtered.iter().filter(|p| p.x >= -8.0 && p.x <= 8.0 && p.y >= 93.0 && p.y <= 102.0 && p.z >= -10.0 && p.z <= 18.0).copied().collect();

  let mut distances: Vec<f64> = in_volume.iter().map(|p| scene.distance_to_scene([p.x, p.y, p.z])).collect();

  distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

  let median_dist = percentile_linear(&distances, 0.50);

  let mean_dist = if distances.is_empty() {
    f64::MAX
  } else {
    distances.iter().sum::<f64>() / distances.len() as f64
  };

  let p90_dist = percentile_linear(&distances, 0.90);

  // Ground plane height std deviation
  let ground_pts: Vec<f64> = in_volume.iter().filter(|p| (p.y - 95.0).abs() < 1.0 && p.z > 0.0).map(|p| p.y).collect();
  let ground_std = if ground_pts.len() >= 2 {
    let mean_y = ground_pts.iter().sum::<f64>() / ground_pts.len() as f64;
    let var = ground_pts.iter().map(|y| (y - mean_y).powi(2)).sum::<f64>() / ground_pts.len() as f64;
    var.sqrt()
  } else {
    0.0
  };

  // Surface completeness: ratio of GT surface samples with at least one point within 0.5m
  let gt_samples = scene.sample_gt_surface_points(0.5);
  let mut covered_count = 0usize;
  for gt_pt in &gt_samples {
    let covered = in_volume.iter().any(|p| {
      let dx = p.x - gt_pt[0];
      let dy = p.y - gt_pt[1];
      let dz = p.z - gt_pt[2];
      (dx * dx + dy * dy + dz * dz) <= 0.25 // 0.5m radius
    });
    if covered {
      covered_count += 1;
    }
  }

  let completeness = if gt_samples.is_empty() {
    0.0
  } else {
    covered_count as f64 / gt_samples.len() as f64
  };

  let passes_median = median_dist < 0.5;
  let passes_completeness = completeness > 0.60;
  let passes_ground = ground_std < 0.20;
  let verdict = if passes_median && passes_completeness && passes_ground {
    "GO"
  } else {
    "NO-GO"
  };

  GeometricMetrics {
    total_points: points.len(),
    filtered_points: filtered.len(),
    filter_opacity_threshold: opacity_thresh,
    points_in_eval_volume: in_volume.len(),
    median_distance_to_gt_m: median_dist,
    mean_distance_to_gt_m: mean_dist,
    p90_distance_to_gt_m: p90_dist,
    surface_completeness_ratio: completeness,
    ground_plane_y_std_m: ground_std,
    passes_median_gate: passes_median,
    passes_completeness_gate: passes_completeness,
    passes_ground_gate: passes_ground,
    final_verdict: verdict.to_string(),
  }
}

pub fn run_evaluator_self_test(scene: &GroundTruthScene) -> Result<GeometricMetrics, String> {
  println!("\n================================================================================");
  println!("           Gate 0 Evaluator Self-Test on Synthetic GT Point Cloud");
  println!("================================================================================");
  let gt_pts = scene.sample_gt_surface_points(0.5);
  println!("[Self-Test] Sampled {} ground truth visible surface points (res=0.5m)", gt_pts.len());

  let synthetic_cloud: Vec<GaussianPoint> = gt_pts
    .iter()
    .map(|p| GaussianPoint {
      x: p[0],
      y: p[1],
      z: p[2],
      opacity: 1.0,
    })
    .collect();

  let metrics = evaluate_point_cloud(&synthetic_cloud, 0.10, scene);

  println!("Total Synthetic Points:           {}", metrics.total_points);
  println!("Points in Evaluation Volume:      {}", metrics.points_in_eval_volume);
  println!(
    "Median Dist to GT Surface:        {:.3} m (Gate: == 0.000m -> {})",
    metrics.median_distance_to_gt_m,
    if metrics.median_distance_to_gt_m.abs() < 1e-4 {
      "PASS"
    } else {
      "FAIL"
    }
  );
  println!("Mean Dist to GT Surface:          {:.3} m", metrics.mean_distance_to_gt_m);
  println!("P90 Dist to GT Surface:           {:.3} m", metrics.p90_distance_to_gt_m);
  println!(
    "Surface Completeness Ratio:       {:.1}% (Gate: == 100.0% -> {})",
    metrics.surface_completeness_ratio * 100.0,
    if (metrics.surface_completeness_ratio - 1.0).abs() < 1e-4 {
      "PASS"
    } else {
      "FAIL"
    }
  );
  println!("Ground Plane Y Std Dev:           {:.3} m", metrics.ground_plane_y_std_m);
  println!("--------------------------------------------------------------------------------");
  let passed = metrics.median_distance_to_gt_m.abs() < 1e-4 && (metrics.surface_completeness_ratio - 1.0).abs() < 1e-4;
  println!("SELF-TEST STATUS:                 {}", if passed { "PASSED" } else { "FAILED" });
  println!("================================================================================\n");

  if metrics.median_distance_to_gt_m.abs() >= 1e-4 {
    return Err(format!("Evaluator self-test FAILED: expected median 0.000m, got {:.3}m", metrics.median_distance_to_gt_m));
  }
  if (metrics.surface_completeness_ratio - 1.0).abs() >= 1e-4 {
    return Err(format!("Evaluator self-test FAILED: expected completeness 100.0%, got {:.1}%", metrics.surface_completeness_ratio * 100.0));
  }

  Ok(metrics)
}

// -----------------------------------------------------------------------------
// Capture & Multi-Pass Execution
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureFrameRecord {
  pub frame_index: usize,
  pub pass_index: usize,
  pub relative_image_path: String,
  pub eye_pos: [f64; 3],
  pub yaw: f64,
  pub pitch: f64,
  pub timestamp_ms: u64,
  pub skew_ms: i64,
}

// -----------------------------------------------------------------------------
// Main CLI
// -----------------------------------------------------------------------------

fn print_banner() {
  println!("\n================================================================================");
  println!("           AUV Trajectory 3D Geometry Spike (traj-geo-spike)");
  println!("================================================================================");
  println!("Crux: Can forward gameplay trajectories yield usable scene geometry via 3DGS?");
  println!("Gate: Median dist < 0.5m AND Surface completeness > 60%");
  println!("Mode: Telemetry as Ground Truth Pose, Zero Production Rust Changes");
  println!("================================================================================\n");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  print_banner();

  let args: Vec<String> = std::env::args().collect();
  let do_setup = args.iter().any(|a| a == "--setup" || a == "--all");
  let do_capture = args.iter().any(|a| a == "--capture" || a == "--all");
  let do_train = args.iter().any(|a| a == "--train" || a == "--all");
  let do_eval = args.iter().any(|a| a == "--eval" || a == "--all");
  let do_depth = args.iter().any(|a| a == "--depth-fusion" || a == "--all");

  let do_self_test = args.iter().any(|a| a == "--self-test");

  let base_dir = PathBuf::from(r"F:\auv\.tmp\traj_geo_spike");
  let dataset_dir = base_dir.join("dataset");
  let images_dir = dataset_dir.join("images");
  let trainer_output_dir = base_dir.join("trainer_output");
  let telemetry_path = PathBuf::from(r"F:\pcl\.minecraft\versions\1.21.1-Fabric 0.16.10\auv\telemetry.jsonl");

  fs::create_dir_all(&images_dir)?;
  fs::create_dir_all(&trainer_output_dir)?;

  // Connect to Driver Session only if live interaction is requested
  let (driver_session, target_window) = if do_setup || do_capture {
    println!("[Init] Connecting to local desktop driver session...");
    let ds = auv_driver::open_local().map_err(|e| format!("Failed to open driver session: {e}"))?;
    let windows = ds.window().list().map_err(|e| format!("Failed to list desktop windows: {e}"))?;
    let tw = match windows.iter().find(|w| {
      if let Some(t) = &w.title {
        if t.to_lowercase().contains("minecraft") || t.contains("新的世界") {
          return true;
        }
      }
      if let Some(a) = &w.app_name {
        if a.to_lowercase().contains("minecraft") || a.to_lowercase().contains("java") {
          return true;
        }
      }
      false
    }) {
      Some(w) => w.clone(),
      None => {
        println!("[Init Error] Visible windows found ({}):", windows.len());
        for w in &windows {
          println!("  - ID={}, app={:?}, title={:?}", w.reference.id, w.app_name, w.title);
        }
        return Err("Could not find active Minecraft window on desktop".into());
      }
    };

    let hwnd = tw.reference.id.parse::<isize>().unwrap_or(0);
    println!("[Init] Connected to Minecraft window: '{}' (HWND {})", tw.title.as_deref().unwrap_or("?"), hwnd);
    win32_input::activate_minecraft(Some(hwnd));
    (Some(ds), Some(tw))
  } else {
    (None, None)
  };

  // ---------------------------------------------------------------------------
  // T1 Step 1: Calibration Scene Setup
  // ---------------------------------------------------------------------------
  if do_setup {
    println!("\n--------------------------------------------------------------------------------");
    println!("Step T1.0: Setting up Ground Truth Calibration Arena in Minecraft");
    println!("--------------------------------------------------------------------------------");

    win32_input::send_chat_command("/time set day");
    win32_input::send_chat_command("/weather clear");
    win32_input::send_chat_command("/gamemode creative");

    // Clear and build ground
    win32_input::send_chat_command("/fill -8 94 -12 8 94 25 minecraft:grass_block");
    win32_input::send_chat_command("/fill -8 95 -12 8 105 25 minecraft:air");

    // GT Structure 1: Stone Wall at Z = -5 (width 5m, height 4m)
    win32_input::send_chat_command("/fill -2 95 -5 2 98 -5 minecraft:stone");

    // GT Structure 2: Cobblestone Step at Z = -3 (width 2m, height 2m)
    win32_input::send_chat_command("/fill 1 95 -3 2 96 -3 minecraft:cobblestone");

    // GT Structure 3: Chests
    win32_input::send_chat_command("/setblock -2 95 -3 minecraft:chest[facing=south]");
    win32_input::send_chat_command("/setblock 0 95 -3 minecraft:chest[facing=south]");

    // Teleport player to Pass 1 Start
    win32_input::send_chat_command("/tp @s 0.5 95.0 12.0 180.0 0.0");
    sleep(Duration::from_millis(1000));
    println!("[Setup] Calibration scene deployed successfully.");
  }

  // ---------------------------------------------------------------------------
  // T1 Step 2: Trajectory Capture (3 Passes, Forward-Dominant with Head-Bob)
  // ---------------------------------------------------------------------------
  let mut captured_frames: Vec<NerfstudioFrame> = Vec::new();
  let mut frame_metadata: Vec<CaptureFrameRecord> = Vec::new();
  let mut baseline_intrinsics: Option<(f64, f64, f64, f64)> = None;
  let mut baseline_viewport: Option<Viewport> = None;

  if do_capture {
    println!("\n--------------------------------------------------------------------------------");
    println!("Step T1.1: Running Multi-Pass Trajectory Capture (>= 3 Passes, Subtle Head-Bob)");
    println!("--------------------------------------------------------------------------------");

    let passes = [
      (1, 0.5, 12.0, 180.0, "Pass 1: Center lane (X=0.5)"),
      (2, 2.0, 12.0, 180.0, "Pass 2: Right lane (X=2.0)"),
      (3, -1.0, 12.0, 180.0, "Pass 3: Left lane (X=-1.0)"),
    ];

    let mut global_frame_idx = 0usize;

    for (pass_idx, start_x, start_z, start_yaw, label) in passes {
      println!("\n--- Starting {label} ---");
      win32_input::send_chat_command(&format!("/tp @s {start_x} 95.0 {start_z} {start_yaw} 0.0"));
      for _ in 0..15 {
        sleep(Duration::from_millis(200));
        if let Ok(Some(f)) = read_latest_spatial_frame_from_tail(&telemetry_path) {
          let dx = f.player_pose.eye_position.x - start_x;
          let dz = f.player_pose.eye_position.z - start_z;
          if (dx * dx + dz * dz).sqrt() < 1.5 {
            println!("  [Pass {pass_idx}] Teleport confirmed at ({:.2}, {:.2})", f.player_pose.eye_position.x, f.player_pose.eye_position.z);
            break;
          }
        }
      }
      sleep(Duration::from_millis(300));

      // Each pass runs 25 ticks: walk forward with subtle head-bob
      for step in 0..25 {
        // A. Subtle intentional head-bob (alternating small yaw wiggles for transverse parallax)
        let bob_dx = if step % 4 == 1 {
          12 // ~ +1.8 deg
        } else if step % 4 == 3 {
          -12 // ~ -1.8 deg
        } else {
          0
        };
        if bob_dx != 0 {
          win32_input::turn_mouse(bob_dx, 0);
          sleep(Duration::from_millis(20));
        }

        // B. Step forward for 180ms
        win32_input::step_forward(Duration::from_millis(180));
        sleep(Duration::from_millis(30));

        // C. Capture Telemetry & Screenshot
        let t_before_cap = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let spatial_frame = read_latest_spatial_frame_from_tail(&telemetry_path)
          .map_err(|e| format!("Failed to read telemetry: {e}"))?
          .ok_or_else(|| "Telemetry file was empty".to_string())?;

        let cap = driver_session
          .as_ref()
          .unwrap()
          .window()
          .capture(target_window.as_ref().unwrap())
          .map_err(|e| format!("Window capture failed: {e}"))?;

        let t_after_cap = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;

        // Check timestamp skew
        let cap_mid_ts = (t_before_cap + t_after_cap) / 2;
        let tele_ts = spatial_frame.monotonic_timestamp_ms;
        // Skew is difference relative to monotonic clock drift
        let skew_ms = (cap_mid_ts as i64 - tele_ts as i64).abs();
        if skew_ms > 100 {
          println!("[Capture Warning] Frame {global_frame_idx} clock delta: {skew_ms}ms (different clock domains)");
        }

        let vp = spatial_frame.viewport;
        let (fl_x, fl_y, cx, cy) = intrinsics_from_projection(vp, &spatial_frame.projection_matrix)
          .ok_or_else(|| "Invalid projection matrix intrinsics".to_string())?;

        if baseline_intrinsics.is_none() {
          baseline_intrinsics = Some((fl_x, fl_y, cx, cy));
          baseline_viewport = Some(vp);
        }

        let w2c = effective_world_to_camera_matrix(&spatial_frame).ok_or_else(|| "Failed to compute world_to_camera matrix".to_string())?;
        let c2w = invert_affine_matrix(&w2c).ok_or_else(|| "Failed to invert affine camera matrix".to_string())?;
        let transform_matrix = matrix_rows(&c2w);

        // Save image
        let img_filename = format!("frame_{global_frame_idx:06}.png");
        let img_full_path = images_dir.join(&img_filename);
        let dyn_img = DynamicImage::ImageRgba8(cap.image);
        dyn_img.save(&img_full_path).map_err(|e| format!("Failed to save image {}: {e}", img_full_path.display()))?;

        captured_frames.push(NerfstudioFrame {
          file_path: format!("images/{img_filename}"),
          transform_matrix,
        });

        let eye = spatial_frame.player_pose.eye_position;
        frame_metadata.push(CaptureFrameRecord {
          frame_index: global_frame_idx,
          pass_index: pass_idx,
          relative_image_path: format!("images/{img_filename}"),
          eye_pos: [eye.x, eye.y, eye.z],
          yaw: spatial_frame.player_pose.yaw,
          pitch: spatial_frame.player_pose.pitch,
          timestamp_ms: tele_ts,
          skew_ms,
        });

        println!(
          "  [Pass {pass_idx} Frame {:02}/25] eye=({:.2}, {:.2}, {:.2}), yaw={:.1}°, saved {}",
          step + 1,
          eye.x,
          eye.y,
          eye.z,
          spatial_frame.player_pose.yaw,
          img_filename
        );

        global_frame_idx += 1;
        sleep(Duration::from_millis(50));
      }
    }

    println!("\n[Capture] Total frames captured: {}", captured_frames.len());

    // Write transforms.json
    let (fl_x, fl_y, cx, cy) = baseline_intrinsics.expect("baseline intrinsics");
    let vp = baseline_viewport.expect("baseline viewport");
    let transforms = NerfstudioTransforms {
      camera_model: "OPENCV".to_string(),
      w: vp.width,
      h: vp.height,
      fl_x,
      fl_y,
      cx,
      cy,
      k1: 0.0,
      k2: 0.0,
      p1: 0.0,
      p2: 0.0,
      frames: captured_frames.clone(),
    };

    let transforms_path = dataset_dir.join("transforms.json");
    let json_str = serde_json::to_string_pretty(&transforms)?;
    fs::write(&transforms_path, json_str)?;
    println!("[Capture] Nerfstudio transforms.json written to {}", transforms_path.display());

    let meta_path = base_dir.join("capture_metadata.json");
    fs::write(&meta_path, serde_json::to_string_pretty(&frame_metadata)?)?;
  }

  // ---------------------------------------------------------------------------
  // T2: 3DGS Reconstruction using Brush
  // ---------------------------------------------------------------------------
  let brush_exe = PathBuf::from(r"F:\auv\.tmp\m4-session\brush\brush.exe");
  let splat_ply_path = trainer_output_dir.join("splat_1000.ply");

  if do_train {
    println!("\n--------------------------------------------------------------------------------");
    println!("Step T2: Running 3DGS Reconstruction (Brush 0.3.0, 1000 steps)");
    println!("--------------------------------------------------------------------------------");

    if !brush_exe.exists() {
      return Err(format!("Brush executable not found at {}", brush_exe.display()).into());
    }

    let start_train = Instant::now();
    println!("[Train] Executing Brush CLI on dataset: {}", dataset_dir.display());

    let status = Command::new(&brush_exe)
      .arg(&dataset_dir)
      .arg("--total-steps")
      .arg("1000")
      .arg("--export-every")
      .arg("1000")
      .arg("--export-path")
      .arg(&trainer_output_dir)
      .arg("--export-name")
      .arg("splat_{iter}.ply")
      .status()
      .map_err(|e| format!("Failed to spawn brush.exe: {e}"))?;

    let train_duration = start_train.elapsed();
    println!("[Train] Brush finished in {:.2}s with status: {:?}", train_duration.as_secs_f64(), status);

    if !splat_ply_path.exists() {
      eprintln!("[Train ERROR] Expected splat file was not created: {}", splat_ply_path.display());
    } else {
      let sz = fs::metadata(&splat_ply_path)?.len();
      println!("[Train] Output splat created: {} ({} bytes)", splat_ply_path.display(), sz);
    }
  }

  // ---------------------------------------------------------------------------
  // T3: Geometric Availability Evaluation
  // ---------------------------------------------------------------------------
  let scene = GroundTruthScene::new_standard();

  if do_self_test || do_eval || do_depth {
    run_evaluator_self_test(&scene)?;
    if do_self_test && !do_eval && !do_depth {
      println!("[Done] Self-test completed successfully.");
      return Ok(());
    }
  }

  if do_eval {
    println!("\n--------------------------------------------------------------------------------");
    println!("Step T3: Geometric Availability Evaluation against Frozen Criteria");
    println!("--------------------------------------------------------------------------------");

    if !splat_ply_path.exists() {
      return Err(format!("Cannot evaluate: splat file not found at {}", splat_ply_path.display()).into());
    }

    println!("[Eval] Parsing Brush PLY Gaussian points...");
    let points = parse_brush_ply(&splat_ply_path)?;
    println!("[Eval] Parsed {} Gaussian centroids from PLY", points.len());

    let metrics = evaluate_point_cloud(&points, 0.10, &scene);

    println!("\n=================== 3DGS GEOMETRIC EVALUATION REPORT ===================");
    println!("Total Gaussian Centroids:         {}", metrics.total_points);
    println!("Opacity >= 0.10 Filtered:         {}", metrics.filtered_points);
    println!("Points in Evaluation Volume:      {}", metrics.points_in_eval_volume);
    println!(
      "Median Dist to GT Surface:        {:.3} m (Gate: < 0.50m -> {})",
      metrics.median_distance_to_gt_m,
      if metrics.passes_median_gate {
        "PASS"
      } else {
        "FAIL"
      }
    );
    println!("Mean Dist to GT Surface:          {:.3} m", metrics.mean_distance_to_gt_m);
    println!("P90 Dist to GT Surface:           {:.3} m", metrics.p90_distance_to_gt_m);
    println!(
      "Surface Completeness Ratio:       {:.1}% (Gate: > 60.0% -> {})",
      metrics.surface_completeness_ratio * 100.0,
      if metrics.passes_completeness_gate {
        "PASS"
      } else {
        "FAIL"
      }
    );
    println!("Ground Plane Y Std Dev:           {:.3} m", metrics.ground_plane_y_std_m);
    println!("------------------------------------------------------------------------");
    println!("FINAL VERDICT:                    {}", metrics.final_verdict);
    println!("========================================================================\n");

    let metrics_path = base_dir.join("3dgs_geometric_metrics.json");
    fs::write(&metrics_path, serde_json::to_string_pretty(&metrics)?)?;
  }

  // ---------------------------------------------------------------------------
  // T4: Depth-Fusion Baseline
  // ---------------------------------------------------------------------------
  if do_depth {
    println!("\n--------------------------------------------------------------------------------");
    println!("Step T4: Running Depth-Fusion Unprojection Baseline");
    println!("--------------------------------------------------------------------------------");

    let depth_model_path = PathBuf::from(r"F:\auv\.tmp\models\depth_anything_v2_vits.onnx");
    if !depth_model_path.exists() {
      println!("[Depth Warning] Depth model not found at {}, skipping depth-fusion", depth_model_path.display());
    } else {
      println!("[Depth] Loading DepthEstimator from {}...", depth_model_path.display());
      let depth_estimator = DepthEstimator::new(&depth_model_path).map_err(|e| format!("Failed to load DepthEstimator: {e}"))?;

      let transforms_path = dataset_dir.join("transforms.json");
      let transforms_content = fs::read_to_string(&transforms_path)?;
      let transforms: NerfstudioTransforms = serde_json::from_str(&transforms_content)?;

      let fl_x = transforms.fl_x;
      let fl_y = transforms.fl_y;
      let cx = transforms.cx;
      let cy = transforms.cy;

      use std::collections::{HashMap, HashSet};

      let voxel_size = 0.25f64;
      let mut voxel_frames: HashMap<(i32, i32, i32), HashSet<usize>> = HashMap::new();
      let start_depth = Instant::now();

      // Track per-frame calibration parameters
      let mut frame_calibs: Vec<(f64, f64)> = Vec::with_capacity(transforms.frames.len());

      for (i, frame) in transforms.frames.iter().enumerate() {
        let img_path = dataset_dir.join(&frame.file_path);
        let dyn_img = image::open(&img_path).map_err(|e| format!("Failed to open frame image {}: {e}", img_path.display()))?;

        let depth_map = depth_estimator.estimate(&dyn_img).map_err(|e| format!("Depth estimation failed: {e}"))?;

        let c2w = frame.transform_matrix;
        let w = depth_map.width as usize;
        let h = depth_map.height as usize;

        // Ground anchor calibration: player eye height above ground is dy = c2w[1][3] - 95.0 (~1.62m)
        let eye_y = c2w[1][3];
        let dy = (eye_y - 95.0).max(0.5);

        // Collect ground samples for OLS disparity affine calibration: 1 / d_gt = s * raw + t
        let mut pairs: Vec<(f64, f64)> = Vec::new();
        for v_s in (320..460).step_by(4) {
          let d_gt = dy * fl_y / (v_s as f64 - cy);
          for u_s in (w / 4..3 * w / 4).step_by(8) {
            let raw = depth_map.values[v_s * w + u_s] as f64;
            if raw > 0.05 {
              pairs.push((raw, d_gt));
            }
          }
        }

        let (s_disp, t_disp) = if pairs.len() >= 2 {
          let n_p = pairs.len() as f64;
          let mean_x = pairs.iter().map(|p| p.0).sum::<f64>() / n_p;
          let mean_y = pairs.iter().map(|p| 1.0 / p.1).sum::<f64>() / n_p;
          let var_x = pairs.iter().map(|p| (p.0 - mean_x).powi(2)).sum::<f64>();
          let cov_xy = pairs.iter().map(|p| (p.0 - mean_x) * (1.0 / p.1 - mean_y)).sum::<f64>();
          if var_x > 1e-8 {
            let s = cov_xy / var_x;
            let t = mean_y - s * mean_x;
            (s, t)
          } else {
            (1.0, 0.0)
          }
        } else {
          (1.0, 0.0)
        };

        frame_calibs.push((s_disp, t_disp));

        if i == 0 {
          println!("[Depth Calibration Frame 0] dy={:.2}m, s={:.6}, t={:.6} (pairs={})", dy, s_disp, t_disp, pairs.len());
        }

        // Subsample pixels: step by 16 for reasonable point cloud density
        let step = 16usize;

        for v in (0..h).step_by(step) {
          for u in (0..w).step_by(step) {
            let raw_d = depth_map.values[v * w + u] as f64;
            if raw_d <= 0.05 {
              continue;
            }

            // Calibrated metric depth via affine disparity model
            let pred_disp = s_disp * raw_d + t_disp;
            if pred_disp <= 0.01 {
              continue;
            }
            let d_metric = 1.0 / pred_disp;
            if d_metric < 0.1 || d_metric > 35.0 {
              continue;
            }

            // OpenGL camera space derived from effective_world_to_camera_matrix / c2w:
            // Right is +X_cam, Up is +Y_cam, View forward is -Z_cam
            let x_cam = (u as f64 - cx) * d_metric / fl_x;
            let y_cam = (cy - v as f64) * d_metric / fl_y;
            let z_cam = -d_metric;

            // Camera-to-world transform: P_w = R * P_c + t
            let xw = c2w[0][0] * x_cam + c2w[0][1] * y_cam + c2w[0][2] * z_cam + c2w[0][3];
            let yw = c2w[1][0] * x_cam + c2w[1][1] * y_cam + c2w[1][2] * z_cam + c2w[1][3];
            let zw = c2w[2][0] * x_cam + c2w[2][1] * y_cam + c2w[2][2] * z_cam + c2w[2][3];

            let vx = (xw / voxel_size).floor() as i32;
            let vy = (yw / voxel_size).floor() as i32;
            let vz = (zw / voxel_size).floor() as i32;

            voxel_frames.entry((vx, vy, vz)).or_default().insert(i);
          }
        }

        if (i + 1) % 15 == 0 || i + 1 == transforms.frames.len() {
          println!("  [Depth-Fusion] Processed {}/{} frames (unique voxels: {})", i + 1, transforms.frames.len(), voxel_frames.len());
        }
      }

      let depth_duration = start_depth.elapsed();
      println!("[Depth-Fusion] 75 frames processed in {:.2}s. Total distinct voxels: {}", depth_duration.as_secs_f64(), voxel_frames.len());

      // Voxel consensus: keep only voxels observed by >= 3 frames
      let consensus_voxels: Vec<GaussianPoint> = voxel_frames
        .into_iter()
        .filter(|(_coord, frames)| frames.len() >= 3)
        .map(|((vx, vy, vz), _frames)| {
          let x = (vx as f64 + 0.5) * voxel_size;
          let y = (vy as f64 + 0.5) * voxel_size;
          let z = (vz as f64 + 0.5) * voxel_size;
          GaussianPoint {
            x,
            y,
            z,
            opacity: 1.0,
          }
        })
        .collect();

      println!("[Voxel Consensus] Voxels observed by >= 3 frames: {}", consensus_voxels.len());

      let depth_metrics = evaluate_point_cloud(&consensus_voxels, 0.5, &scene);

      println!("\n================ DEPTH-FUSION GEOMETRIC EVALUATION REPORT ================");
      println!("Total Backprojected Points:       {}", depth_metrics.total_points);
      println!("Points in Evaluation Volume:      {}", depth_metrics.points_in_eval_volume);
      println!(
        "Median Dist to GT Surface:        {:.3} m (Gate: < 0.50m -> {})",
        depth_metrics.median_distance_to_gt_m,
        if depth_metrics.passes_median_gate {
          "PASS"
        } else {
          "FAIL"
        }
      );
      println!("Mean Dist to GT Surface:          {:.3} m", depth_metrics.mean_distance_to_gt_m);
      println!("P90 Dist to GT Surface:           {:.3} m", depth_metrics.p90_distance_to_gt_m);
      println!(
        "Surface Completeness Ratio:       {:.1}% (Gate: > 60.0% -> {})",
        depth_metrics.surface_completeness_ratio * 100.0,
        if depth_metrics.passes_completeness_gate {
          "PASS"
        } else {
          "FAIL"
        }
      );
      println!("Ground Plane Y Std Dev:           {:.3} m", depth_metrics.ground_plane_y_std_m);
      println!("------------------------------------------------------------------------");
      println!("FINAL VERDICT:                    {}", depth_metrics.final_verdict);
      println!("========================================================================\n");

      let depth_metrics_path = base_dir.join("depth_fusion_geometric_metrics.json");
      fs::write(&depth_metrics_path, serde_json::to_string_pretty(&depth_metrics)?)?;
    }
  }

  println!("\n[Done] Trajectory 3D Geometry Spike completed successfully.");
  Ok(())
}
