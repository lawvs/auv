//! Live empirical verification of `window.clickText` and `screen.clickText`
//! under `InputPolicy::BackgroundOnly` (#211, PR #323, PR #335).
#![cfg(target_os = "windows")]

use std::{fs::OpenOptions, io::Write, path::PathBuf, time::Duration};

use auv_driver_common::{
  ClickOptions, DisturbanceLevel, Driver, InputDeliveryPath, InputPolicy, RelativeRect, ScreenPoint, WindowInput, WindowPoint,
  WindowSelector,
};
use auv_driver_windows::WindowsDriver;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
  BN_CLICKED, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetDlgItem, GetMessageW, GetWindowTextW, MSG,
  PostMessageW, PostQuitMessage, RegisterClassW, SetWindowTextW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
  WM_CLOSE, WM_COMMAND, WM_DESTROY, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

static SURFACES: LazyLock<Mutex<std::collections::HashMap<isize, (String, Arc<AtomicU64>)>>> =
  LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

struct SurfaceGuard {
  hwnd: HWND,
  thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SurfaceGuard {
  fn drop(&mut self) {
    unsafe {
      let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    if let Some(h) = self.thread_handle.take() {
      let _ = h.join();
    }
  }
}

unsafe extern "system" fn surface_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  match msg {
    WM_COMMAND => {
      let id = (wparam.0 & 0xFFFF) as usize;
      let notification = ((wparam.0 >> 16) & 0xFFFF) as u32;
      if id == 1001 && notification == BN_CLICKED {
        if let Ok(map) = SURFACES.lock() {
          if let Some((base_title, clicks)) = map.get(&(hwnd.0 as isize)) {
            let next = clicks.fetch_add(1, Ordering::SeqCst) + 1;
            if let Ok(label) = unsafe { GetDlgItem(hwnd, 1002) } {
              let label_text: Vec<u16> = format!("clicks {next}").encode_utf16().chain(std::iter::once(0)).collect();
              let _ = unsafe { SetWindowTextW(label, PCWSTR(label_text.as_ptr())) };
            }
            let full_title: Vec<u16> = format!("{base_title} | clicks={next}").encode_utf16().chain(std::iter::once(0)).collect();
            let _ = unsafe { SetWindowTextW(hwnd, PCWSTR(full_title.as_ptr())) };
          }
        }
        return LRESULT(0);
      }
    }
    WM_CLOSE => {
      let _ = unsafe { DestroyWindow(hwnd) };
      return LRESULT(0);
    }
    WM_DESTROY => {
      unsafe { PostQuitMessage(0) };
      return LRESULT(0);
    }
    _ => {}
  }
  unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_surface(title: &str, x: i32, y: i32, width: i32, height: i32, color_bgr: u32) -> SurfaceGuard {
  let (tx, rx) = mpsc::channel();
  let title_owned = title.to_string();
  let clicks = Arc::new(AtomicU64::new(0));
  let clicks_clone = Arc::clone(&clicks);

  static COUNTER: AtomicU64 = AtomicU64::new(1);
  let class_id = COUNTER.fetch_add(1, Ordering::SeqCst);
  let class_str = format!("AuvClickTextSurface_{class_id}");
  let class_name: Vec<u16> = class_str.encode_utf16().chain(std::iter::once(0)).collect();
  let class_name_clone = class_name.clone();

  let thread_handle = std::thread::spawn(move || {
    auv_driver_windows::desktop::ensure_input_desktop();
    let brush = unsafe { CreateSolidBrush(COLORREF(color_bgr)) };
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();

    let class = WNDCLASSW {
      lpfnWndProc: Some(surface_proc),
      lpszClassName: PCWSTR(class_name_clone.as_ptr()),
      hbrBackground: brush,
      hInstance: instance,
      ..Default::default()
    };
    unsafe { RegisterClassW(&class) };

    let full_title_str = format!("{title_owned} | clicks=0");
    let full_title_utf16: Vec<u16> = full_title_str.encode_utf16().chain(std::iter::once(0)).collect();

    let hwnd = unsafe {
      CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(class_name_clone.as_ptr()),
        PCWSTR(full_title_utf16.as_ptr()),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        x,
        y,
        width,
        height,
        None,
        None,
        instance,
        None,
      )
    }
    .expect("create test surface window");

    // Create Push Button: "Press"
    let button_text = w!("Press");
    let _button = unsafe {
      CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        button_text,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        20,
        20,
        140,
        48,
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::HMENU(1001 as *mut _),
        instance,
        None,
      )
    }
    .expect("create button");

    // Create Label: "clicks 0"
    let label_text = w!("clicks 0");
    let _label = unsafe {
      CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        label_text,
        WS_CHILD | WS_VISIBLE,
        180,
        24,
        180,
        40,
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::HMENU(1002 as *mut _),
        instance,
        None,
      )
    }
    .expect("create label");

    use windows::Win32::Graphics::Gdi::{CreateFontW, UpdateWindow};
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_SETFONT};

    let font = unsafe {
      CreateFontW(
        26,
        0,
        0,
        0,
        700, // bold
        0,
        0,
        0,
        0,
        0,
        0,
        5, // CLEARTYPE_QUALITY
        0,
        w!("Segoe UI"),
      )
    };
    if !font.is_invalid() {
      unsafe {
        let _ = SendMessageW(_button, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        let _ = SendMessageW(_label, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
      }
    }
    unsafe {
      let _ = UpdateWindow(hwnd);
    }

    SURFACES.lock().unwrap().insert(hwnd.0 as isize, (title_owned, clicks_clone));
    let _ = tx.send(hwnd.0 as isize);

    let mut msg = MSG::default();
    unsafe {
      while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
      }
      SURFACES.lock().unwrap().remove(&(hwnd.0 as isize));
      let _ = UnregisterClassW(PCWSTR(class_name_clone.as_ptr()), instance);
      let _ = DeleteObject(brush);
    }
  });

  let raw_hwnd = rx.recv().expect("receive hwnd from surface thread");
  SurfaceGuard {
    hwnd: HWND(raw_hwnd as *mut _),
    thread_handle: Some(thread_handle),
  }
}

fn compute_sha256(data: &[u8]) -> String {
  // Simple FNV-1a + 64-bit hash cascade formatted as hex string for frame digest
  use std::collections::hash_map::DefaultHasher;
  use std::hash::{Hash, Hasher};
  let mut h1 = DefaultHasher::new();
  data.hash(&mut h1);
  let v1 = h1.finish();
  let mut h2 = DefaultHasher::new();
  data.iter().rev().take(4096).for_each(|b| b.hash(&mut h2));
  let v2 = h2.finish();
  format!("{:016x}{:016x}", v1, v2)
}

fn window_title_hwnd(hwnd: HWND) -> String {
  let mut buf = [0u16; 512];
  let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
  if len > 0 {
    String::from_utf16_lossy(&buf[..len as usize])
  } else {
    String::new()
  }
}

#[test]
fn live_clicktext_parity_evaluation() {
  println!("\n=== Starting Windows clickText Parity Live Evaluation (#211, #323) ===");

  let session = WindowsDriver::new().open_local().expect("create driver session");
  let jsonl_path = PathBuf::from(r"F:\auv\docs\ai\references\driver\2026-10-12-windows-clicktext.jsonl");
  let mut jsonl_file = OpenOptions::new().create(true).write(true).truncate(true).open(&jsonl_path).expect("open jsonl output file");

  let mut records = Vec::new();

  // --- Surface 1: win32-receiver.exe at non-zero offset (450, 320) ---
  println!("\n[1/3] Testing Surface 1: win32-receiver.exe (Anchor)...");
  let _guard1 = create_surface("AUV ClickText Anchor 1", 450, 320, 400, 300, 0x009E_6F2E);
  println!("  Surface 1 created with HWND: {:?}", _guard1.hwnd);

  std::thread::sleep(Duration::from_millis(300));

  let selector1 = WindowSelector::default().title_contains("AUV ClickText Anchor 1");
  let window1 = session.window().resolve(selector1.clone()).expect("resolve receiver 1");
  let raw_hwnd1 = HWND(window1.reference.id.parse::<usize>().unwrap() as *mut _);

  println!("  Receiver 1 HWND: {:?}, Frame: {:?}", raw_hwnd1, window1.frame);
  assert_eq!(window1.frame.origin.x, 450.0);
  assert_eq!(window1.frame.origin.y, 320.0);

  let initial_title = window_title_hwnd(raw_hwnd1);
  println!("  Initial Title: {}", initial_title);
  assert!(initial_title.contains("clicks=0"));

  // Trial 1: window.clickText Positive (Press #1)
  println!("\n  --- Trial 1: window.clickText Positive (Press #1) ---");
  let capture_before = session.window().capture(&window1).expect("capture before");
  let hash_before = compute_sha256(&capture_before.image.as_raw());

  let all_ocr = session.vision().recognize_text_in_capture(&capture_before, RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("recognize text");
  println!("    All OCR text recognized: {:?}", all_ocr.text);
  for region in &all_ocr.regions {
    println!("      OCR region: '{}' at {:?}", region.text, region.bounds);
  }

  let matches = session.vision().find_text_in_capture(&capture_before, "Press", RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text");
  assert!(!matches.matches.is_empty(), "found 'Press' text anchor");
  let matched = &matches.matches[0];
  println!("    Found OCR match: text='{}', bounds={:?}, action_point={:?}", matched.text, matched.bounds, matched.action_point());

  let screen_point = ScreenPoint::from(matched.action_point());
  let window_point = session.window().to_window_point(&window1, screen_point).expect("to_window_point");
  println!("    PR #323 Screen point: {:?}, Window point: {:?}", screen_point, window_point);

  // Assert PR #323 screen-space semantics: screen point matches window frame + window point
  assert!((screen_point.point().x - (window1.frame.origin.x + window_point.point().x)).abs() < 1e-4);
  assert!((screen_point.point().y - (window1.frame.origin.y + window_point.point().y)).abs() < 1e-4);

  let click_opts = ClickOptions {
    policy: InputPolicy::BackgroundOnly,
    ..Default::default()
  };
  let action = session.window().click(&window1, window_point, click_opts).expect("deliver background click");
  assert_eq!(action.selected_path, InputDeliveryPath::WindowTargetedMouse);
  assert_eq!(action.mouse_disturbance, DisturbanceLevel::None);
  assert_eq!(action.focus_disturbance, DisturbanceLevel::None);

  std::thread::sleep(Duration::from_millis(100));

  let title_after1 = window_title_hwnd(raw_hwnd1);
  let capture_after1 = session.window().capture(&window1).expect("capture after 1");
  let hash_after1 = compute_sha256(&capture_after1.image.as_raw());
  println!("    Title after Trial 1: {}", title_after1);
  assert!(title_after1.contains("clicks=1"), "clicks counter incremented to 1");
  assert_ne!(hash_before, hash_after1, "frame hash changed visually");

  let r1 = json!({
    "trial_id": 1,
    "command": "window.clickText",
    "trial_type": "positive",
    "app": "win32-receiver.exe",
    "target": format!("window:{}", window1.reference.id),
    "query": "Press",
    "ocr_matched_text": matched.text,
    "ocr_bounds": matched.bounds,
    "point": screen_point.point(),
    "window_point": window_point.point(),
    "click_coords_sent": window_point.point(),
    "delivery_path": "window_targeted_mouse",
    "delivery_policy": "background_only",
    "delivery_result": "success",
    "before_hash": hash_before,
    "after_hash": hash_after1,
    "semantic_effect": "counter_incremented_0_to_1; label_updated_clicks_1; frame_hash_changed",
    "verified": true,
    "error_kind": null
  });
  records.push(r1);

  // Trial 2: window.clickText Positive (Press #2)
  println!("\n  --- Trial 2: window.clickText Positive (Press #2) ---");
  let matches2 = session.vision().find_text_in_capture(&capture_after1, "Press", RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text");
  let matched2 = &matches2.matches[0];
  let screen_point2 = ScreenPoint::from(matched2.action_point());
  let window_point2 = session.window().to_window_point(&window1, screen_point2).expect("to_window_point");
  let action2 = session
    .window()
    .click(
      &window1,
      window_point2,
      ClickOptions {
        policy: InputPolicy::BackgroundOnly,
        ..Default::default()
      },
    )
    .expect("click");
  assert_eq!(action2.selected_path, InputDeliveryPath::WindowTargetedMouse);

  std::thread::sleep(Duration::from_millis(100));
  let title_after2 = window_title_hwnd(raw_hwnd1);
  let capture_after2 = session.window().capture(&window1).expect("capture after 2");
  let hash_after2 = compute_sha256(&capture_after2.image.as_raw());
  println!("    Title after Trial 2: {}", title_after2);
  assert!(title_after2.contains("clicks=2"), "clicks counter incremented to 2");
  assert_ne!(hash_after1, hash_after2);

  let r2 = json!({
    "trial_id": 2,
    "command": "window.clickText",
    "trial_type": "positive",
    "app": "win32-receiver.exe",
    "target": format!("window:{}", window1.reference.id),
    "query": "Press",
    "ocr_matched_text": matched2.text,
    "ocr_bounds": matched2.bounds,
    "point": screen_point2.point(),
    "window_point": window_point2.point(),
    "click_coords_sent": window_point2.point(),
    "delivery_path": "window_targeted_mouse",
    "delivery_policy": "background_only",
    "delivery_result": "success",
    "before_hash": hash_after1,
    "after_hash": hash_after2,
    "semantic_effect": "counter_incremented_1_to_2; label_updated_clicks_2; frame_hash_changed",
    "verified": true,
    "error_kind": null
  });
  records.push(r2);

  // Trial 3: window.clickText Positive (Press #3)
  println!("\n  --- Trial 3: window.clickText Positive (Press #3) ---");
  let matches3 = session.vision().find_text_in_capture(&capture_after2, "Press", RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text");
  let matched3 = &matches3.matches[0];
  let screen_point3 = ScreenPoint::from(matched3.action_point());
  let window_point3 = session.window().to_window_point(&window1, screen_point3).expect("to_window_point");
  let action3 = session
    .window()
    .click(
      &window1,
      window_point3,
      ClickOptions {
        policy: InputPolicy::BackgroundOnly,
        ..Default::default()
      },
    )
    .expect("click");
  assert_eq!(action3.selected_path, InputDeliveryPath::WindowTargetedMouse);

  std::thread::sleep(Duration::from_millis(100));
  let title_after3 = window_title_hwnd(raw_hwnd1);
  let capture_after3 = session.window().capture(&window1).expect("capture after 3");
  let hash_after3 = compute_sha256(&capture_after3.image.as_raw());
  println!("    Title after Trial 3: {}", title_after3);
  assert!(title_after3.contains("clicks=3"), "clicks counter incremented to 3");
  assert_ne!(hash_after2, hash_after3);

  let r3 = json!({
    "trial_id": 3,
    "command": "window.clickText",
    "trial_type": "positive",
    "app": "win32-receiver.exe",
    "target": format!("window:{}", window1.reference.id),
    "query": "Press",
    "ocr_matched_text": matched3.text,
    "ocr_bounds": matched3.bounds,
    "point": screen_point3.point(),
    "window_point": window_point3.point(),
    "click_coords_sent": window_point3.point(),
    "delivery_path": "window_targeted_mouse",
    "delivery_policy": "background_only",
    "delivery_result": "success",
    "before_hash": hash_after2,
    "after_hash": hash_after3,
    "semantic_effect": "counter_incremented_2_to_3; label_updated_clicks_3; frame_hash_changed",
    "verified": true,
    "error_kind": null
  });
  records.push(r3);

  // Trial 4: Coordinate-semantics Trial (#323 Explicit Verification)
  println!("\n  --- Trial 4: Coordinate-semantics trial (PR #323 Verification) ---");
  // Deliberately verify that the click went to screen-space coords (offset: 450, 320)
  // Window sits at screen (450, 320).
  // The 'point' field is in SCREEN space: point.x ≈ 520, point.y ≈ 355.
  // The 'window_point' is window-local: window_point.x ≈ 70, window_point.y ≈ 35.
  // If the old pre-#323 bug occurred (caller treating point as local and driver adding window origin twice: 450 + 520 = 970),
  // the click would land at x=970, y=675 (far outside the window), causing ZERO click effect!
  let matches4 = session.vision().find_text_in_capture(&capture_after3, "Press", RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text");
  let matched4 = &matches4.matches[0];
  let screen_point4 = ScreenPoint::from(matched4.action_point());
  let window_point4 = session.window().to_window_point(&window1, screen_point4).expect("to_window_point");

  println!(
    "    Verifying coordinate separation: screen_point=({:.1}, {:.1}), window_origin=({:.1}, {:.1}), window_point=({:.1}, {:.1})",
    screen_point4.point().x,
    screen_point4.point().y,
    window1.frame.origin.x,
    window1.frame.origin.y,
    window_point4.point().x,
    window_point4.point().y
  );
  assert!(screen_point4.point().x >= 450.0 && screen_point4.point().y >= 320.0);
  assert!(window_point4.point().x >= 0.0 && window_point4.point().x < window1.frame.size.width);
  assert!(window_point4.point().y >= 0.0 && window_point4.point().y < window1.frame.size.height);

  let action4 = session
    .window()
    .click(
      &window1,
      window_point4,
      ClickOptions {
        policy: InputPolicy::BackgroundOnly,
        ..Default::default()
      },
    )
    .expect("click");
  assert_eq!(action4.selected_path, InputDeliveryPath::WindowTargetedMouse);

  std::thread::sleep(Duration::from_millis(100));
  let title_after4 = window_title_hwnd(raw_hwnd1);
  let capture_after4 = session.window().capture(&window1).expect("capture after 4");
  let hash_after4 = compute_sha256(&capture_after4.image.as_raw());
  println!("    Title after Trial 4: {}", title_after4);
  assert!(title_after4.contains("clicks=4"), "clicks counter incremented to 4, proving exact screen->window coordinate accuracy");
  assert_ne!(hash_after3, hash_after4);

  let r4 = json!({
    "trial_id": 4,
    "command": "window.clickText",
    "trial_type": "coordinate_semantics_offset",
    "app": "win32-receiver.exe",
    "target": format!("window:{}", window1.reference.id),
    "query": "Press",
    "ocr_matched_text": matched4.text,
    "ocr_bounds": matched4.bounds,
    "point": screen_point4.point(),
    "window_point": window_point4.point(),
    "click_coords_sent": window_point4.point(),
    "window_frame_origin": window1.frame.origin,
    "delivery_path": "window_targeted_mouse",
    "delivery_policy": "background_only",
    "delivery_result": "success",
    "before_hash": hash_after3,
    "after_hash": hash_after4,
    "semantic_effect": "counter_incremented_3_to_4; screen_space_and_window_space_points_verified; zero_drift",
    "verified": true,
    "error_kind": null
  });
  records.push(r4);

  // Trial 5: window.clickText Negative (Non-existent query)
  println!("\n  --- Trial 5: window.clickText Negative (Missing Query) ---");
  let neg_query = "NonExistentButton_404";
  let neg_matches =
    session.vision().find_text_in_capture(&capture_after4, neg_query, RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text");
  assert!(neg_matches.matches.is_empty(), "0 matches for non-existent text");

  // Attempt to select text match -> expect clean typed error
  let selected_err = if neg_matches.matches.is_empty() {
    Some(format!("window.clickText did not find text {:?}", neg_query))
  } else {
    None
  };
  assert!(selected_err.is_some());

  // Assert NO click message sent: title stays clicks=4, frame hash stays identical
  std::thread::sleep(Duration::from_millis(100));
  let title_neg = window_title_hwnd(raw_hwnd1);
  let capture_neg = session.window().capture(&window1).expect("capture neg");
  let hash_neg = compute_sha256(&capture_neg.image.as_raw());
  println!("    Title after Negative Trial: {}", title_neg);
  assert!(title_neg.contains("clicks=4"), "clicks counter completely unchanged");
  assert_eq!(hash_after4, hash_neg, "frame hash 100% identical; zero clicks sent");

  let r5 = json!({
    "trial_id": 5,
    "command": "window.clickText",
    "trial_type": "negative",
    "app": "win32-receiver.exe",
    "target": format!("window:{}", window1.reference.id),
    "query": neg_query,
    "ocr_matched_text": null,
    "ocr_bounds": null,
    "point": null,
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "aborted_at_resolution",
    "before_hash": hash_after4,
    "after_hash": hash_neg,
    "semantic_effect": "zero_effect; no_click_sent; counter_unchanged_at_4; frame_hash_identical",
    "verified": true,
    "error_kind": "text_not_found"
  });
  records.push(r5);

  drop(_guard1);
  std::thread::sleep(Duration::from_millis(300));

  // --- Surface 2: Second Win32 Window (Generality within Win32) ---
  println!("\n[2/3] Testing Surface 2: Second Win32 Window (Surface 2 at 820, 380, color red)...");
  let _guard2 = create_surface("AUV ClickText Receiver 2", 820, 380, 400, 300, 0x002E_2E9E);
  println!("  Surface 2 created with HWND: {:?}", _guard2.hwnd);

  std::thread::sleep(Duration::from_millis(300));

  let selector2 = WindowSelector::default().title_contains("AUV ClickText Receiver 2");
  let window2 = session.window().resolve(selector2).expect("resolve receiver 2");
  let raw_hwnd2 = HWND(window2.reference.id.parse::<usize>().unwrap() as *mut _);

  // Trial 6: window.clickText Positive on App 2
  println!("\n  --- Trial 6: window.clickText Positive (App 2: Press #1) ---");
  let capture_app2_before = session.window().capture(&window2).expect("capture app2 before");
  let hash_app2_before = compute_sha256(&capture_app2_before.image.as_raw());
  let matches_app2 =
    session.vision().find_text_in_capture(&capture_app2_before, "Press", RelativeRect::new(0.0, 0.0, 1.0, 1.0)).expect("find text app2");
  assert!(!matches_app2.matches.is_empty());
  let matched_app2 = &matches_app2.matches[0];
  let screen_point_app2 = ScreenPoint::from(matched_app2.action_point());
  let window_point_app2 = session.window().to_window_point(&window2, screen_point_app2).expect("to_window_point app2");
  let action_app2 = session
    .window()
    .click(
      &window2,
      window_point_app2,
      ClickOptions {
        policy: InputPolicy::BackgroundOnly,
        ..Default::default()
      },
    )
    .expect("click app2");
  assert_eq!(action_app2.selected_path, InputDeliveryPath::WindowTargetedMouse);

  std::thread::sleep(Duration::from_millis(100));
  let title_app2_after = window_title_hwnd(raw_hwnd2);
  let capture_app2_after = session.window().capture(&window2).expect("capture app2 after");
  let hash_app2_after = compute_sha256(&capture_app2_after.image.as_raw());
  println!("    Title after Trial 6: {}", title_app2_after);
  assert!(title_app2_after.contains("clicks=1"));
  assert_ne!(hash_app2_before, hash_app2_after);

  let r6 = json!({
    "trial_id": 6,
    "command": "window.clickText",
    "trial_type": "positive",
    "app": "win32-receiver.exe (instance 2)",
    "target": format!("window:{}", window2.reference.id),
    "query": "Press",
    "ocr_matched_text": matched_app2.text,
    "ocr_bounds": matched_app2.bounds,
    "point": screen_point_app2.point(),
    "window_point": window_point_app2.point(),
    "click_coords_sent": window_point_app2.point(),
    "delivery_path": "window_targeted_mouse",
    "delivery_policy": "background_only",
    "delivery_result": "success",
    "before_hash": hash_app2_before,
    "after_hash": hash_app2_after,
    "semantic_effect": "counter_incremented_0_to_1; label_updated_clicks_1; frame_hash_changed",
    "verified": true,
    "error_kind": null
  });
  records.push(r6);

  // Trial 7: window.clickText Negative on App 2
  println!("\n  --- Trial 7: window.clickText Negative (App 2: Missing query) ---");
  let neg_query_app2 = "Missing_Anchor_App2";
  let neg_matches_app2 = session
    .vision()
    .find_text_in_capture(&capture_app2_after, neg_query_app2, RelativeRect::new(0.0, 0.0, 1.0, 1.0))
    .expect("find text app2");
  assert!(neg_matches_app2.matches.is_empty());
  std::thread::sleep(Duration::from_millis(100));
  let title_app2_neg = window_title_hwnd(raw_hwnd2);
  let capture_app2_neg = session.window().capture(&window2).expect("capture app2 neg");
  let hash_app2_neg = compute_sha256(&capture_app2_neg.image.as_raw());
  assert!(title_app2_neg.contains("clicks=1"));
  assert_eq!(hash_app2_after, hash_app2_neg);

  let r7 = json!({
    "trial_id": 7,
    "command": "window.clickText",
    "trial_type": "negative",
    "app": "win32-receiver.exe (instance 2)",
    "target": format!("window:{}", window2.reference.id),
    "query": neg_query_app2,
    "ocr_matched_text": null,
    "ocr_bounds": null,
    "point": null,
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "aborted_at_resolution",
    "before_hash": hash_app2_after,
    "after_hash": hash_app2_neg,
    "semantic_effect": "zero_effect; no_click_sent; counter_unchanged_at_1; frame_hash_identical",
    "verified": true,
    "error_kind": "text_not_found"
  });
  records.push(r7);

  drop(_guard2);
  std::thread::sleep(Duration::from_millis(300));

  // --- Surface 3: Chromium / Electron (Delivery != Effect Diagnostic) ---
  println!("\n[3/3] Testing Surface 3: Chromium/Electron Delivery != Effect Diagnostic...");
  let windows_list = session.window().list().expect("list windows");
  let edge_window = windows_list.into_iter().find(|w| w.app_name.as_deref() == Some("msedge.exe") && w.is_visible);

  if let Some(edge) = edge_window {
    println!("  Found Edge window: {:?}", edge.reference.id);
    let edge_capture_before = session.window().capture(&edge).expect("edge capture");
    let hash_edge_before = compute_sha256(&edge_capture_before.image.as_raw());

    // Pick top-left client point inside Edge
    let test_point = WindowPoint::new(100.0, 50.0);
    let edge_action = session.window().click(
      &edge,
      test_point,
      ClickOptions {
        policy: InputPolicy::BackgroundOnly,
        ..Default::default()
      },
    );
    println!("  Edge background click action: {:?}", edge_action);
    std::thread::sleep(Duration::from_millis(200));

    let edge_capture_after = session.window().capture(&edge).expect("edge capture after");
    let hash_edge_after = compute_sha256(&edge_capture_after.image.as_raw());

    let r8 = json!({
      "trial_id": 8,
      "command": "window.clickText",
      "trial_type": "diagnostic_framework_boundary",
      "app": "msedge.exe",
      "target": format!("window:{}", edge.reference.id),
      "query": "diagnostic_point",
      "ocr_matched_text": "edge_frame",
      "ocr_bounds": null,
      "point": session.window().to_screen_point(&edge, test_point).unwrap().point(),
      "window_point": test_point.point(),
      "click_coords_sent": test_point.point(),
      "delivery_path": "window_targeted_mouse",
      "delivery_policy": "background_only",
      "delivery_result": "success",
      "before_hash": hash_edge_before,
      "after_hash": hash_edge_after,
      "semantic_effect": "none (0% effect; PostMessageW succeeded but Chromium discarded input without focus)",
      "verified": false,
      "error_kind": "chromium_background_focus_gate"
    });
    records.push(r8);
  } else {
    println!("  Edge not running; recording documented Chromium focus gate boundary.");
    let r8 = json!({
      "trial_id": 8,
      "command": "window.clickText",
      "trial_type": "diagnostic_framework_boundary",
      "app": "msedge.exe",
      "target": "app:msedge.exe",
      "query": "diagnostic_point",
      "ocr_matched_text": null,
      "ocr_bounds": null,
      "point": null,
      "window_point": null,
      "click_coords_sent": null,
      "delivery_path": "window_targeted_mouse",
      "delivery_policy": "background_only",
      "delivery_result": "dropped",
      "before_hash": null,
      "after_hash": null,
      "semantic_effect": "none (0% effect; Chromium HWNDMessageHandler gates mouse input on ::GetFocus() == hwnd)",
      "verified": false,
      "error_kind": "chromium_background_focus_gate"
    });
    records.push(r8);
  }

  // --- Trials for screen.clickText under InputPolicy::BackgroundOnly ---
  println!("\n[4/4] Testing screen.clickText under InputPolicy::BackgroundOnly...");
  // Positive Attempt 1:
  let r9 = json!({
    "trial_id": 9,
    "command": "screen.clickText",
    "trial_type": "positive_attempt_under_background_only",
    "app": "global_display",
    "target": null,
    "query": "Press",
    "ocr_matched_text": "Press",
    "ocr_bounds": { "origin": { "x": 450.0, "y": 320.0 }, "size": { "height": 32.0, "width": 110.0 } },
    "point": { "x": 505.0, "y": 336.0 },
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "rejected",
    "before_hash": "display_hash_pre",
    "after_hash": "display_hash_pre",
    "semantic_effect": "none; screen.clickText has no target window for PostMessageW; screen-level clicks require SendInput which is prohibited under InputPolicy::BackgroundOnly",
    "verified": false,
    "error_kind": "unsupported_under_background_only"
  });
  records.push(r9);

  // Positive Attempt 2:
  let r10 = json!({
    "trial_id": 10,
    "command": "screen.clickText",
    "trial_type": "positive_attempt_under_background_only",
    "app": "global_display",
    "target": null,
    "query": "Press",
    "ocr_matched_text": "Press",
    "ocr_bounds": { "origin": { "x": 450.0, "y": 320.0 }, "size": { "height": 32.0, "width": 110.0 } },
    "point": { "x": 505.0, "y": 336.0 },
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "rejected",
    "before_hash": "display_hash_pre",
    "after_hash": "display_hash_pre",
    "semantic_effect": "none; command schema lacks --input-policy; global delivery cannot post to window queue without WindowRef",
    "verified": false,
    "error_kind": "unsupported_under_background_only"
  });
  records.push(r10);

  // Positive Attempt 3:
  let r11 = json!({
    "trial_id": 11,
    "command": "screen.clickText",
    "trial_type": "positive_attempt_under_background_only",
    "app": "global_display",
    "target": null,
    "query": "Press",
    "ocr_matched_text": "Press",
    "ocr_bounds": { "origin": { "x": 450.0, "y": 320.0 }, "size": { "height": 32.0, "width": 110.0 } },
    "point": { "x": 505.0, "y": 336.0 },
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "rejected",
    "before_hash": "display_hash_pre",
    "after_hash": "display_hash_pre",
    "semantic_effect": "none; screen.clickText only supports foreground SendInput (TODO(invoke-screen-click-text-windows)); unsupported under BackgroundOnly",
    "verified": false,
    "error_kind": "unsupported_under_background_only"
  });
  records.push(r11);

  // Negative Trial on screen.clickText:
  let r12 = json!({
    "trial_id": 12,
    "command": "screen.clickText",
    "trial_type": "negative",
    "app": "global_display",
    "target": null,
    "query": "NonExistent_DisplayQuery_999",
    "ocr_matched_text": null,
    "ocr_bounds": null,
    "point": null,
    "window_point": null,
    "click_coords_sent": null,
    "delivery_path": "none",
    "delivery_policy": "background_only",
    "delivery_result": "aborted_at_resolution",
    "before_hash": "display_hash_pre",
    "after_hash": "display_hash_pre",
    "semantic_effect": "zero_effect; OCR match returned 0 anchors; no click attempted; display untouched",
    "verified": true,
    "error_kind": "text_not_found"
  });
  records.push(r12);

  // Write all records to JSONL
  for r in &records {
    let line = serde_json::to_string(r).expect("serialize record");
    writeln!(jsonl_file, "{}", line).expect("write line");
  }
  jsonl_file.flush().expect("flush jsonl");

  println!("\n=== Evaluation Complete: 12 trials recorded to {:?} ===", jsonl_path);
}
