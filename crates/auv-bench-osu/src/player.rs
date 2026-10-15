use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::injector::{PrecisionClock, Win32InputActuator, percentile_linear};
use crate::model::BeatmapModel;
use crate::projection::{PlayfieldProjection, ProjectionCalibrationReport};
use crate::synthesizer::{ActionPlan, KeyCode, KeyEventType};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LivePlayEdgeRecord {
  pub edge_index: usize,
  pub planned_time_ms: f64,
  pub actual_time_ms: f64,
  pub raw_deviation_ms: f64,
  pub key: KeyCode,
  pub event_type: KeyEventType,
  pub target_pos_pf: (f64, f64),
  pub target_pos_screen: (f64, f64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LivePlayReport {
  pub map_name: String,
  pub lazer_version: String,
  pub calibration: ProjectionCalibrationReport,
  pub planned_edges: usize,
  pub actual_edges: usize,
  pub count_reconciled: bool,
  pub raw_p50_ms: f64,
  pub raw_p95_ms: f64,
  pub raw_p99_ms: f64,
  pub raw_max_ms: f64,
  pub comp_p50_ms: f64,
  pub comp_p95_ms: f64,
  pub comp_p99_ms: f64,
  pub comp_max_ms: f64,
  pub duration_seconds: f64,
  pub records: Vec<LivePlayEdgeRecord>,
}

#[derive(Clone, Copy, Debug)]
pub struct LivePlayerConfig {
  pub window_width: f64,
  pub window_height: f64,
  pub lead_in_delay_ms: f64,
  pub click_retry: bool,
}

impl Default for LivePlayerConfig {
  fn default() -> Self {
    Self {
      window_width: 2560.0,
      window_height: 1440.0,
      lead_in_delay_ms: 0.0,
      click_retry: true,
    }
  }
}

#[cfg(target_os = "windows")]
pub fn focus_osu_window() {
  unsafe {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::StationsAndDesktops::{
      CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, OpenInputDesktop, SetThreadDesktop,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
      BringWindowToTop, EnumWindows, GetClassNameW, GetWindowTextW, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let mut target_hwnd = HWND(std::ptr::null_mut());
    let target_ptr = &mut target_hwnd as *mut HWND;

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
      unsafe {
        let mut title = [0u16; 256];
        let mut class = [0u16; 256];
        let _ = GetWindowTextW(hwnd, &mut title);
        let _ = GetClassNameW(hwnd, &mut class);
        let title_str = String::from_utf16_lossy(&title);
        let _class_str = String::from_utf16_lossy(&class);
        // Exclude other SDL apps like Steam (title='Menu'), strictly match osu! title
        if title_str.starts_with("osu!") {
          let out_ptr = lparam.0 as *mut HWND;
          *out_ptr = hwnd;
          return BOOL(0);
        }
        BOOL(1)
      }
    }

    if let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0x000F_01FF)) {
      let _ = SetThreadDesktop(desk);
      let _ = EnumWindows(Some(enum_proc), LPARAM(target_ptr as isize));

      if !target_hwnd.0.is_null() {
        let _ = ShowWindow(target_hwnd, SW_RESTORE);
        let _ = BringWindowToTop(target_hwnd);
        let _ = SetForegroundWindow(target_hwnd);
      }
      let _ = CloseDesktop(desk);
    }
  }
}

#[cfg(target_os = "windows")]
pub fn check_user_disturbance(last_sent_pos: Option<(f64, f64)>, tolerance_px: f64) -> Option<&'static str> {
  use windows::Win32::Foundation::POINT;
  use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
  use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowTextW};

  unsafe {
    // 1. Esc key abort
    let esc = GetAsyncKeyState(0x1B);
    if (esc as u16 & 0x8000) != 0 {
      return Some("User pressed Escape key");
    }

    // 2. Foreground window lost abort
    let fg = GetForegroundWindow();
    let mut title = [0u16; 256];
    let _ = GetWindowTextW(fg, &mut title);
    let title_str = String::from_utf16_lossy(&title);
    if !title_str.starts_with("osu!") {
      return Some("Target window (osu!) lost foreground focus");
    }

    // 3. Physical mouse moved abort
    if let Some((sx, sy)) = last_sent_pos {
      let mut pt = POINT { x: 0, y: 0 };
      if GetCursorPos(&mut pt).is_ok() {
        let dx = (pt.x as f64) - sx;
        let dy = (pt.y as f64) - sy;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > tolerance_px {
          return Some("Physical mouse movement detected");
        }
      }
    }
  }
  None
}

#[cfg(not(target_os = "windows"))]
pub fn check_user_disturbance(_last_sent_pos: Option<(f64, f64)>, _tolerance_px: f64) -> Option<&'static str> {
  None
}

/// Find the most recently written osu! runtime log in %APPDATA%\osu\logs
pub fn find_active_osu_runtime_log() -> Option<PathBuf> {
  let appdata = std::env::var("APPDATA").ok()?;
  let logs_dir = PathBuf::from(appdata).join("osu").join("logs");
  let read_dir = std::fs::read_dir(logs_dir).ok()?;

  let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
  for entry in read_dir.flatten() {
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()) == Some("log")
      && path.file_name().and_then(|n| n.to_str()).map(|n| n.ends_with(".runtime.log")).unwrap_or(false)
      && let Ok(meta) = entry.metadata()
      && let Ok(mtime) = meta.modified()
      && best.as_ref().map(|b| mtime > b.1).unwrap_or(true)
    {
      best = Some((path, mtime));
    }
  }
  best.map(|b| b.0)
}

/// Wait for osu! to write "GameplayClockContainer started via call to StartGameplayClock" to the runtime log.
/// Also extracts the lead-in duration from "GameplayClockContainer seeking to -(\d+)".
pub fn wait_for_gameplay_clock_start(log_path: &Path, initial_pos: u64, clock: &PrecisionClock, timeout_ms: u64) -> (f64, Option<f64>) {
  let mut file = match std::fs::File::open(log_path) {
    Ok(f) => f,
    Err(e) => {
      eprintln!("Warning: Failed to open runtime log {:?}: {e}", log_path);
      return (clock.elapsed_ms(), None);
    }
  };

  let _ = file.seek(SeekFrom::Start(initial_pos));
  let start_wait = clock.elapsed_ms();
  let mut detected_lead_in: Option<f64> = None;
  let mut buf = Vec::new();

  loop {
    if let Some(reason) = check_user_disturbance(None, 60.0) {
      eprintln!("\n[FAILSAFE ABORT] Aborted during clock synchronization: {reason}");
      return (clock.elapsed_ms(), detected_lead_in);
    }

    if (clock.elapsed_ms() - start_wait) > (timeout_ms as f64) {
      eprintln!(
        "Notice: Timeout waiting for StartGameplayClock in log (waited {:.0}ms). Falling back to current clock.",
        clock.elapsed_ms() - start_wait
      );
      return (clock.elapsed_ms(), detected_lead_in);
    }

    let mut chunk = [0u8; 4096];
    match file.read(&mut chunk) {
      Ok(n) if n > 0 => {
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);

        if detected_lead_in.is_none()
          && let Some(pos) = text.find("GameplayClockContainer seeking to -")
        {
          let rem = &text[pos + "GameplayClockContainer seeking to -".len()..];
          let num_str: String = rem.chars().take_while(|c| c.is_ascii_digit()).collect();
          if let Ok(lead) = num_str.parse::<f64>() {
            detected_lead_in = Some(lead);
            println!("[Sync] Detected beatmap seeking lead-in: -{lead:.0}ms");
          }
        }

        if text.contains("GameplayClockContainer started via call to StartGameplayClock") {
          let t_start = clock.elapsed_ms();
          println!("[Sync] GameplayClockContainer started detected at t_wall={:.2}ms", t_start);
          return (t_start, detected_lead_in);
        }
      }
      _ => {
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
    }
  }
}

/// Executes a synthesized ActionPlan live via Win32 SendInput on a dedicated desktop thread.
pub fn execute_live_play(
  model: &BeatmapModel,
  plan: &ActionPlan,
  config: &LivePlayerConfig,
  calib_report: ProjectionCalibrationReport,
) -> Result<LivePlayReport> {
  let proj = PlayfieldProjection::for_capture(config.window_width, config.window_height, model.difficulty.circle_size)
    .map_err(|e| anyhow::anyhow!("projection error: {e}"))?;

  let width = config.window_width;
  let height = config.window_height;
  let plan_clone = plan.clone();
  let cfg = *config;

  let active_log = find_active_osu_runtime_log();
  let initial_log_len = active_log.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);

  // Run actuator loop on a dedicated thread bound to WinSta0\Default
  let handle = std::thread::spawn(move || -> Result<Vec<LivePlayEdgeRecord>> {
    #[cfg(target_os = "windows")]
    unsafe {
      use windows::Win32::System::StationsAndDesktops::{DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, OpenInputDesktop, SetThreadDesktop};
      if let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0x000F_01FF)) {
        let _ = SetThreadDesktop(desk);
      }
    }

    if cfg.click_retry {
      #[cfg(target_os = "windows")]
      focus_osu_window();
      std::thread::sleep(std::time::Duration::from_millis(200));

      // 1. Try Result Screen retry button at bottom center (1400, 1395)
      let norm_res_x = ((1400.0 / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      let norm_res_y = ((1395.0 / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      Win32InputActuator::send_mouse_pos(1400, 1395, norm_res_x, norm_res_y);
      std::thread::sleep(std::time::Duration::from_millis(50));
      Win32InputActuator::send_mouse_click();
      std::thread::sleep(std::time::Duration::from_millis(50));

      // 2. Try Pause Screen retry at (1280, 500)
      let norm_pause_x = ((1280.0 / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      let norm_pause_y = ((500.0 / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      Win32InputActuator::send_mouse_pos(1280, 500, norm_pause_x, norm_pause_y);
      std::thread::sleep(std::time::Duration::from_millis(50));
      Win32InputActuator::send_mouse_click();
      std::thread::sleep(std::time::Duration::from_millis(50));

      // 3. Try Failure Screen retry at (1280, 595)
      let norm_fail_x = ((1280.0 / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      let norm_fail_y = ((595.0 / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      Win32InputActuator::send_mouse_pos(1280, 595, norm_fail_x, norm_fail_y);
      std::thread::sleep(std::time::Duration::from_millis(50));
      Win32InputActuator::send_mouse_click();
      std::thread::sleep(std::time::Duration::from_millis(50));

      // 4. Send Ctrl+R quick retry and Enter
      Win32InputActuator::send_quick_retry();
      Win32InputActuator::send_virtual_key_press(0x0D); // VK_RETURN
    }

    let clock = PrecisionClock::new();

    // Synchronize clock with osu! runtime log
    let (t_start, detected_lead_in) = if cfg.click_retry {
      if let Some(ref path) = active_log {
        println!("[Sync] Synchronizing with osu! gameplay clock via {:?}...", path.file_name().unwrap_or_default());
        wait_for_gameplay_clock_start(path, initial_log_len, &clock, 5000)
      } else {
        (clock.elapsed_ms(), None)
      }
    } else {
      (clock.elapsed_ms(), None)
    };

    let actual_lead_in = if cfg.lead_in_delay_ms > 0.0 {
      cfg.lead_in_delay_ms
    } else if let Some(lead) = detected_lead_in {
      lead
    } else if cfg.click_retry {
      1148.0
    } else {
      0.0
    };

    println!("[Sync] Clock locked: t_start={:.2}ms, lead_in={:.2}ms", t_start, actual_lead_in);

    let num_edges = plan_clone.key_schedule.len();
    let mut records = Vec::with_capacity(num_edges);

    let mut key_idx = 0;
    let mut traj_idx = 0;

    let keys = &plan_clone.key_schedule;
    let traj = &plan_clone.trajectory;

    let mut last_sent_screen_pos: Option<(f64, f64)> = None;

    // Move cursor to initial position immediately upon clock sync
    if let Some(first_pt) = traj.first() {
      let (wx, wy) = proj.to_window_point(first_pt.x, first_pt.y);
      let norm_x = ((wx / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      let norm_y = ((wy / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
      Win32InputActuator::send_mouse_pos(wx as i32, wy as i32, norm_x, norm_y);
      last_sent_screen_pos = Some((wx, wy));
    }

    let mut aborted = false;
    let mut abort_reason = "";

    while key_idx < keys.len() || traj_idx < traj.len() {
      // Find the next event time between trajectory and key schedule
      let next_traj_time = traj.get(traj_idx).map(|p| p.time_ms);
      let next_key_time = keys.get(key_idx).map(|k| k.time_ms);

      match (next_traj_time, next_key_time) {
        (Some(tt), Some(kt)) if tt <= kt => {
          // Trajectory step
          let target_wall_time = t_start + actual_lead_in + tt;
          if let Err(reason) = clock.sleep_until_ms_with_cancel(target_wall_time, || check_user_disturbance(last_sent_screen_pos, 60.0)) {
            aborted = true;
            abort_reason = reason;
            break;
          }

          let pt = &traj[traj_idx];
          let (wx, wy) = proj.to_window_point(pt.x, pt.y);
          let norm_x = ((wx / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
          let norm_y = ((wy / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
          Win32InputActuator::send_mouse_pos(wx as i32, wy as i32, norm_x, norm_y);
          last_sent_screen_pos = Some((wx, wy));
          traj_idx += 1;
        }
        (Some(_), Some(kt)) => {
          // Key event step
          let target_wall_time = t_start + actual_lead_in + kt;
          if let Err(reason) = clock.sleep_until_ms_with_cancel(target_wall_time, || check_user_disturbance(last_sent_screen_pos, 60.0)) {
            aborted = true;
            abort_reason = reason;
            break;
          }
          let actual_time = clock.elapsed_ms() - t_start - actual_lead_in;

          let ev = &keys[key_idx];
          let is_down = ev.event_type == KeyEventType::Down;
          Win32InputActuator::send_key(ev.key, is_down);

          // Find current cursor position in plan
          let current_pf_pos = traj.get(traj_idx).map(|p| (p.x, p.y)).unwrap_or((256.0, 192.0));
          let current_screen_pos = proj.to_window_point(current_pf_pos.0, current_pf_pos.1);

          records.push(LivePlayEdgeRecord {
            edge_index: key_idx,
            planned_time_ms: kt,
            actual_time_ms: actual_time,
            raw_deviation_ms: actual_time - kt,
            key: ev.key,
            event_type: ev.event_type,
            target_pos_pf: current_pf_pos,
            target_pos_screen: current_screen_pos,
          });
          key_idx += 1;
        }
        (Some(tt), None) => {
          let target_wall_time = t_start + actual_lead_in + tt;
          if let Err(reason) = clock.sleep_until_ms_with_cancel(target_wall_time, || check_user_disturbance(last_sent_screen_pos, 60.0)) {
            aborted = true;
            abort_reason = reason;
            break;
          }

          let pt = &traj[traj_idx];
          let (wx, wy) = proj.to_window_point(pt.x, pt.y);
          let norm_x = ((wx / width) * 65535.0).round().clamp(0.0, 65535.0) as u16;
          let norm_y = ((wy / height) * 65535.0).round().clamp(0.0, 65535.0) as u16;
          Win32InputActuator::send_mouse_pos(wx as i32, wy as i32, norm_x, norm_y);
          last_sent_screen_pos = Some((wx, wy));
          traj_idx += 1;
        }
        (None, Some(kt)) => {
          let target_wall_time = t_start + actual_lead_in + kt;
          if let Err(reason) = clock.sleep_until_ms_with_cancel(target_wall_time, || check_user_disturbance(last_sent_screen_pos, 60.0)) {
            aborted = true;
            abort_reason = reason;
            break;
          }
          let actual_time = clock.elapsed_ms() - t_start - actual_lead_in;

          let ev = &keys[key_idx];
          let is_down = ev.event_type == KeyEventType::Down;
          Win32InputActuator::send_key(ev.key, is_down);

          let current_pf_pos = (256.0, 192.0);
          let current_screen_pos = proj.to_window_point(current_pf_pos.0, current_pf_pos.1);

          records.push(LivePlayEdgeRecord {
            edge_index: key_idx,
            planned_time_ms: kt,
            actual_time_ms: actual_time,
            raw_deviation_ms: actual_time - kt,
            key: ev.key,
            event_type: ev.event_type,
            target_pos_pf: current_pf_pos,
            target_pos_screen: current_screen_pos,
          });
          key_idx += 1;
        }
        (None, None) => break,
      }
    }

    // Always release all keys at the end of play or abort
    Win32InputActuator::send_key(KeyCode::K1, false);
    Win32InputActuator::send_key(KeyCode::K2, false);

    if aborted {
      eprintln!("\n[FAILSAFE ABORT] Execution halted by user disturbance: {abort_reason}");
    }

    Ok(records)
  });

  let records = handle.join().map_err(|_| anyhow::anyhow!("player thread panicked"))??;

  let planned_edges = plan.key_schedule.len();
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

  let duration_seconds = records.last().map(|r| r.actual_time_ms / 1000.0).unwrap_or(0.0);

  Ok(LivePlayReport {
    map_name: format!("{} - {} [{}]", plan.header.map_artist, plan.header.map_title, plan.header.map_version),
    lazer_version: "2026.1005.0-lazer".to_string(),
    calibration: calib_report,
    planned_edges,
    actual_edges,
    count_reconciled,
    raw_p50_ms: raw_p50,
    raw_p95_ms: raw_p95,
    raw_p99_ms: raw_p99,
    raw_max_ms: raw_max,
    comp_p50_ms: comp_p50,
    comp_p95_ms: comp_p95,
    comp_p99_ms: comp_p99,
    comp_max_ms: comp_max,
    duration_seconds,
    records,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  #[ignore = "live interaction test"]
  fn test_select_insane_live() {
    focus_osu_window();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let width = 2560.0f64;
    let height = 1440.0f64;
    let nx = ((1750.0f64 / width) * 65535.0f64).round() as u16;
    let ny = ((645.0f64 / height) * 65535.0f64).round() as u16;
    Win32InputActuator::send_mouse_pos(1750, 645, nx, ny);
    std::thread::sleep(std::time::Duration::from_millis(100));
    Win32InputActuator::send_mouse_click();
  }
}
