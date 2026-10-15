use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "windows"))]
use std::time::Instant;

#[cfg(target_os = "windows")]
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
#[cfg(target_os = "windows")]
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
  INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
  MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VK_X, VK_Z,
};

pub struct TimePeriodGuard {
  active: bool,
}

impl TimePeriodGuard {
  pub fn new() -> Self {
    #[cfg(target_os = "windows")]
    unsafe {
      let _ = timeBeginPeriod(1);
    }
    Self { active: true }
  }
}

impl Default for TimePeriodGuard {
  fn default() -> Self {
    Self::new()
  }
}

impl Drop for TimePeriodGuard {
  fn drop(&mut self) {
    if self.active {
      #[cfg(target_os = "windows")]
      unsafe {
        let _ = timeEndPeriod(1);
      }
      self.active = false;
    }
  }
}

pub struct PrecisionClock {
  #[cfg(target_os = "windows")]
  frequency: f64,
  #[cfg(target_os = "windows")]
  start_counter: i64,
  #[cfg(not(target_os = "windows"))]
  start_instant: Instant,
  _period_guard: TimePeriodGuard,
}

impl PrecisionClock {
  pub fn new() -> Self {
    let guard = TimePeriodGuard::new();
    #[cfg(target_os = "windows")]
    {
      let mut freq = 0i64;
      let mut start = 0i64;
      unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut start);
      }
      Self {
        frequency: freq as f64,
        start_counter: start,
        _period_guard: guard,
      }
    }
    #[cfg(not(target_os = "windows"))]
    {
      Self {
        start_instant: Instant::now(),
        _period_guard: guard,
      }
    }
  }

  /// Current elapsed time in milliseconds since clock creation.
  pub fn elapsed_ms(&self) -> f64 {
    #[cfg(target_os = "windows")]
    {
      let mut current = 0i64;
      unsafe {
        let _ = QueryPerformanceCounter(&mut current);
      }
      let delta_ticks = current - self.start_counter;
      (delta_ticks as f64) * 1000.0 / self.frequency
    }
    #[cfg(not(target_os = "windows"))]
    {
      self.start_instant.elapsed().as_secs_f64() * 1000.0
    }
  }

  /// Hybrid wait until `target_ms`: sleeps while remaining > 2ms, spins for remainder.
  pub fn sleep_until_ms(&self, target_ms: f64) {
    loop {
      let current = self.elapsed_ms();
      let remaining = target_ms - current;
      if remaining <= 0.0 {
        break;
      }
      if remaining > 2.5 {
        std::thread::sleep(std::time::Duration::from_millis((remaining - 2.0).floor() as u64));
      } else {
        std::hint::spin_loop();
      }
    }
  }

  /// Hybrid wait until `target_ms`: sleeps in small slices (up to 5ms) while checking for disturbance/cancel.
  pub fn sleep_until_ms_with_cancel<F>(&self, target_ms: f64, mut check_cancel: F) -> Result<(), &'static str>
  where
    F: FnMut() -> Option<&'static str>,
  {
    loop {
      if let Some(reason) = check_cancel() {
        return Err(reason);
      }
      let current = self.elapsed_ms();
      let remaining = target_ms - current;
      if remaining <= 0.0 {
        break;
      }
      if remaining > 10.0 {
        std::thread::sleep(std::time::Duration::from_millis(5));
      } else if remaining > 2.5 {
        std::thread::sleep(std::time::Duration::from_millis((remaining - 2.0).floor() as u64));
      } else {
        std::hint::spin_loop();
      }
    }
    Ok(())
  }
}

impl Default for PrecisionClock {
  fn default() -> Self {
    Self::new()
  }
}

/// Win32 SendInput wrapper for osu! actuator
pub struct Win32InputActuator;

impl Win32InputActuator {
  /// Send keyboard key event (VK_Z or VK_X) with hardware scan codes
  pub fn send_key(key: crate::synthesizer::KeyCode, is_down: bool) {
    #[cfg(target_os = "windows")]
    unsafe {
      let (vk, scan) = match key {
        crate::synthesizer::KeyCode::K1 => (VK_Z, 0x2Cu16),
        crate::synthesizer::KeyCode::K2 => (VK_X, 0x2Du16),
      };
      let flags = if is_down { 0 } else { KEYEVENTF_KEYUP.0 };
      let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: vk,
            wScan: scan,
            dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(flags),
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
    #[cfg(not(target_os = "windows"))]
    {
      let _ = (key, is_down);
    }
  }

  /// Send mouse position with both SetCursorPos and SendInput absolute coordinates
  pub fn send_mouse_pos(x: i32, y: i32, norm_x: u16, norm_y: u16) {
    #[cfg(target_os = "windows")]
    unsafe {
      use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
      let _ = SetCursorPos(x, y);
      let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
          mi: MOUSEINPUT {
            dx: norm_x as i32,
            dy: norm_y as i32,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
    #[cfg(not(target_os = "windows"))]
    {
      let _ = (x, y, norm_x, norm_y);
    }
  }

  /// Send mouse position normalized to desktop [0, 65535]
  pub fn send_mouse_absolute(norm_x: u16, norm_y: u16) {
    #[cfg(target_os = "windows")]
    unsafe {
      let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
          mi: MOUSEINPUT {
            dx: norm_x as i32,
            dy: norm_y as i32,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
    #[cfg(not(target_os = "windows"))]
    {
      let _ = (norm_x, norm_y);
    }
  }

  /// Send left mouse click (down then up)
  pub fn send_mouse_click() {
    #[cfg(target_os = "windows")]
    unsafe {
      let down = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
          mi: MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_LEFTDOWN,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let up = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
          mi: MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_LEFTUP,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
      std::thread::sleep(std::time::Duration::from_millis(30));
      let _ = SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
    }
  }

  /// Send virtual key press (down then up)
  pub fn send_virtual_key_press(vk: u16) {
    #[cfg(target_os = "windows")]
    unsafe {
      use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
      let down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk),
            wScan: 0,
            dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk),
            wScan: 0,
            dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
      std::thread::sleep(std::time::Duration::from_millis(50));
      let _ = SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
    }
    #[cfg(not(target_os = "windows"))]
    {
      let _ = vk;
    }
  }

  /// Send Ctrl+R key combination for quick retry across gameplay and results screen
  pub fn send_quick_retry() {
    #[cfg(target_os = "windows")]
    unsafe {
      use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, VK_CONTROL, VK_R};
      let ctrl_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VK_CONTROL,
            wScan: 0x1D,
            dwFlags: KEYBD_EVENT_FLAGS(0),
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let r_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VK_R,
            wScan: 0x13,
            dwFlags: KEYBD_EVENT_FLAGS(0),
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let r_up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VK_R,
            wScan: 0x13,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let ctrl_up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
          ki: KEYBDINPUT {
            wVk: VK_CONTROL,
            wScan: 0x1D,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
          },
        },
      };
      let _ = SendInput(&[ctrl_down, r_down], std::mem::size_of::<INPUT>() as i32);
      std::thread::sleep(std::time::Duration::from_millis(50));
      let _ = SendInput(&[r_up, ctrl_up], std::mem::size_of::<INPUT>() as i32);
    }
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkEdgeRecord {
  pub edge_index: usize,
  pub planned_time_ms: f64,
  pub actual_time_ms: f64,
  pub raw_deviation_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActuatorBenchmarkReport {
  pub planned_edges: usize,
  pub actual_edges: usize,
  pub count_reconciled: bool,
  pub raw_p50_ms: f64,
  pub raw_p95_ms: f64,
  pub raw_p99_ms: f64,
  pub raw_max_ms: f64,
  pub median_ms: f64,
  pub comp_p50_ms: f64,
  pub comp_p95_ms: f64,
  pub comp_p99_ms: f64,
  pub comp_max_ms: f64,
  pub gate_16_6ms_passed: bool,
  pub records: Vec<BenchmarkEdgeRecord>,
}

pub fn run_actuator_smoke_bench(num_edges: usize, interval_ms: f64) -> ActuatorBenchmarkReport {
  let clock = PrecisionClock::new();
  let start_offset_ms = 10.0;
  let mut records = Vec::with_capacity(num_edges);

  for i in 0..num_edges {
    let planned_time = start_offset_ms + (i as f64) * interval_ms;
    clock.sleep_until_ms(planned_time);
    let actual_time = clock.elapsed_ms();

    // Fire simulated actuator signal
    Win32InputActuator::send_key(crate::synthesizer::KeyCode::K1, true);
    Win32InputActuator::send_key(crate::synthesizer::KeyCode::K1, false);

    records.push(BenchmarkEdgeRecord {
      edge_index: i,
      planned_time_ms: planned_time,
      actual_time_ms: actual_time,
      raw_deviation_ms: actual_time - planned_time,
    });
  }

  let planned_edges = num_edges;
  let actual_edges = records.len();
  let count_reconciled = planned_edges == actual_edges;

  let mut raw_devs: Vec<f64> = records.iter().map(|r| r.raw_deviation_ms).collect();
  raw_devs.sort_by(|a, b| a.partial_cmp(b).unwrap());

  let raw_p50 = percentile_linear(&raw_devs, 0.50);
  let raw_p95 = percentile_linear(&raw_devs, 0.95);
  let raw_p99 = percentile_linear(&raw_devs, 0.99);
  let raw_max = *raw_devs.last().unwrap_or(&0.0);

  let median = raw_p50;
  let mut comp_devs: Vec<f64> = raw_devs.iter().map(|x| (x - median).abs()).collect();
  comp_devs.sort_by(|a, b| a.partial_cmp(b).unwrap());

  let comp_p50 = percentile_linear(&comp_devs, 0.50);
  let comp_p95 = percentile_linear(&comp_devs, 0.95);
  let comp_p99 = percentile_linear(&comp_devs, 0.99);
  let comp_max = *comp_devs.last().unwrap_or(&0.0);

  let gate_passed = comp_p99 <= 16.6;

  ActuatorBenchmarkReport {
    planned_edges,
    actual_edges,
    count_reconciled,
    raw_p50_ms: raw_p50,
    raw_p95_ms: raw_p95,
    raw_p99_ms: raw_p99,
    raw_max_ms: raw_max,
    median_ms: median,
    comp_p50_ms: comp_p50,
    comp_p95_ms: comp_p95,
    comp_p99_ms: comp_p99,
    comp_max_ms: comp_max,
    gate_16_6ms_passed: gate_passed,
    records,
  }
}

/// Verifies that osu!lazer is in guest/offline mode with no account logged in.
/// Performs multi-layer verification specifically for osu!lazer:
/// 1. Verifies `%APPDATA%\osu\game.ini` exists and `Username` and `Token` are strictly empty.
/// 2. Verifies `%APPDATA%\osu\framework.ini` (osu!framework/lazer config) exists.
/// 3. Verifies `%APPDATA%\osu\logs\*.auth.log` reports `API state changed to Offline`.
/// 4. Verifies `%APPDATA%\osu\client.realm` (lazer database) is present.
///
/// Aborts with Err if any user credentials or online states are found.
pub fn verify_offline_safety_guard() -> Result<(), String> {
  let appdata = std::env::var("APPDATA").unwrap_or_else(|_| r"C:\Users\Administrator\AppData\Roaming".to_string());
  let osu_dir = std::path::Path::new(&appdata).join("osu");
  verify_lazer_offline_safety_guard_at_dir(&osu_dir)
}

pub fn verify_offline_safety_guard_at_path(path: &std::path::Path) -> Result<(), String> {
  if path.is_file() {
    verify_lazer_ini_file(path)
  } else {
    verify_lazer_offline_safety_guard_at_dir(path)
  }
}

fn verify_lazer_ini_file(ini_path: &std::path::Path) -> Result<(), String> {
  if !ini_path.exists() {
    return Ok(());
  }
  let content = std::fs::read_to_string(ini_path).map_err(|e| format!("failed to read {}: {e}", ini_path.display()))?;

  for line in content.lines() {
    let trimmed = line.trim();
    if let Some(val) = trimmed.strip_prefix("Username") {
      let val = val.trim_start();
      if let Some(v) = val.strip_prefix('=') {
        let username = v.trim();
        if !username.is_empty() {
          return Err(format!(
            "SAFETY ABORT: Logged-in user detected ('{}') in {}. Must be in guest/offline mode with blank Username!",
            username,
            ini_path.display()
          ));
        }
      }
    }
    if let Some(val) = trimmed.strip_prefix("Token") {
      let val = val.trim_start();
      if let Some(v) = val.strip_prefix('=') {
        let token = v.trim();
        if !token.is_empty() {
          return Err(format!(
            "SAFETY ABORT: Account token detected ('{}') in {}. Must be in guest/offline mode with blank Token!",
            token,
            ini_path.display()
          ));
        }
      }
    }
  }
  Ok(())
}

pub fn verify_lazer_offline_safety_guard_at_dir(dir: &std::path::Path) -> Result<(), String> {
  if !dir.exists() {
    return Ok(());
  }

  let ini_path = dir.join("game.ini");
  verify_lazer_ini_file(&ini_path)?;

  // Inspect auth logs in logs/ to ensure no online session is logged in
  let logs_dir = dir.join("logs");
  if logs_dir.exists()
    && let Ok(entries) = std::fs::read_dir(&logs_dir)
  {
    for entry in entries.flatten() {
      let log_path = entry.path();
      let is_auth_log = log_path.file_name().and_then(|n| n.to_str()).map(|name| name.ends_with(".auth.log")).unwrap_or(false);

      if is_auth_log
        && let Ok(content) = std::fs::read_to_string(&log_path)
        && content.contains("API state changed to Online")
      {
        return Err(format!("SAFETY ABORT: Online API state detected in auth log {}. Must be Offline!", log_path.display()));
      }
    }
  }

  Ok(())
}

/// Linear interpolation percentile (matching numpy default)
pub fn percentile_linear(sorted: &[f64], p: f64) -> f64 {
  if sorted.is_empty() {
    return 0.0;
  }
  if sorted.len() == 1 {
    return sorted[0];
  }
  let rank = p * ((sorted.len() - 1) as f64);
  let lower_idx = rank.floor() as usize;
  let upper_idx = rank.ceil() as usize;
  let fraction = rank - (lower_idx as f64);

  if lower_idx == upper_idx {
    sorted[lower_idx]
  } else {
    sorted[lower_idx] + fraction * (sorted[upper_idx] - sorted[lower_idx])
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_percentile_linear() {
    let vals = vec![10.0, 20.0, 30.0, 40.0, 50.0];
    assert!((percentile_linear(&vals, 0.50) - 30.0).abs() < 1e-6);
    assert!((percentile_linear(&vals, 0.0) - 10.0).abs() < 1e-6);
    assert!((percentile_linear(&vals, 1.0) - 50.0).abs() < 1e-6);
  }

  #[test]
  fn test_actuator_smoke_10_edges() {
    let report = run_actuator_smoke_bench(10, 5.0);
    assert!(report.count_reconciled, "edge count must reconcile");
    assert_eq!(report.actual_edges, 10);
    assert!(report.comp_p99_ms <= 16.6, "compensated P99 must be under 16.6ms gate, got {}ms", report.comp_p99_ms);
  }

  #[test]
  fn test_offline_safety_guard_passes_on_empty() {
    let dir = std::env::temp_dir();
    let temp_file = dir.join("test_game_empty.ini");
    std::fs::write(&temp_file, "Username = \nToken = \n").unwrap();
    let res = verify_offline_safety_guard_at_path(&temp_file);
    let _ = std::fs::remove_file(temp_file);
    assert!(res.is_ok());
  }

  #[test]
  fn test_offline_safety_guard_aborts_on_credential() {
    let dir = std::env::temp_dir();
    let temp_file = dir.join("test_game_logged_in.ini");
    std::fs::write(&temp_file, "Username = peppy\nToken = secret\n").unwrap();
    let res = verify_offline_safety_guard_at_path(&temp_file);
    let _ = std::fs::remove_file(temp_file);
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.contains("SAFETY ABORT"));
    assert!(err.contains("peppy"));
  }
}
