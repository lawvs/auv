# Windows `clickText` Parity & Coordinate Semantics (#211, PR #323, PR #335)

## 1. Executive Summary

This report establishes empirical evidence for `window.clickText` and `screen.clickText` on Windows under `InputPolicy::BackgroundOnly`.

Following the completion of targeted keyboard input scoping (PR #349) and paste text delivery (PR #341), this evaluation assesses the composition of OCR text localization (`findText`) and background mouse delivery (`PostMessageW` via `ChildWindowFromPointEx`).

### Key Findings
1. **`window.clickText` on Win32 Controls**: **Proven & Functional (100% Semantic Effect)**.
   - The end-to-end pipeline (`OCR capture` → `text bounding box` → `action point calculation` → `to_window_point` → `ChildWindowFromPointEx` → `PostMessageW(WM_LBUTTONDOWN/UP)`) operates with **0.0px systematic offset**.
   - Clicks reliably trigger Win32 button notifications (`BN_CLICKED`), increment application state counters, update UI label text, and change frame hashes across sequential positive trials without stealing foreground focus.
   - When given non-existent queries, `window.clickText` cleanly aborts with `text_not_found` and emits **zero click messages**, preserving frame hashes and application state.
2. **PR #323 Coordinate Semantics**: **100% Verified**.
   - In PR #323, `window.clickText`'s `point` was updated to represent **screen-space** coordinates, while `window_point` holds **window-local** coordinates.
   - Trials with non-zero window screen offsets (`450, 320` and `820, 380`) confirm that `point == window.origin + window_point`.
   - The caller passes `window_point` to `session.window().click`, which correctly translates to client space. No double-offset bug occurs.
3. **Chromium / Electron Framework Boundary**: **Delivery != Effect (0% Semantic Effect)**.
   - Just as observed with `typeText` and `wheel`, posting `WM_LBUTTONDOWN` / `WM_LBUTTONUP` to Chromium / Electron (Microsoft Edge) returns `TRUE` from the Win32 message subsystem, but Chromium's `HWNDMessageHandler` discards mouse input when `::GetFocus() != hwnd`.
4. **`screen.clickText` under `BackgroundOnly`**: **Unsupported by Architectural Design**.
   - `screen.clickText` operates globally across displays and lacks a target `WindowRef`.
   - Without a destination HWND, background `PostMessageW` cannot be addressed.
   - Screen-level delivery requires `SendInput` (hardware foreground mouse movement and clicking), which violates `InputPolicy::BackgroundOnly`.

Raw trial records are documented in [`docs/ai/references/driver/2026-10-12-windows-clicktext.jsonl`](docs/ai/references/driver/2026-10-12-windows-clicktext.jsonl).

---

## 2. PR #323 Coordinate Semantics Verification

### 2.1 The Coordinate Semantic Contract
PR #323 migrated `window.clickText` to separate screen-space coordinates from window-local coordinates:
- `point`: **`ScreenPoint`** (absolute desktop pixel coordinates).
- `window_point`: **`WindowPoint`** (coordinates relative to the target window frame origin).

### 2.2 Live Offset Verification Trial (Trial 4)
To verify that neither the caller nor the driver applies double-offsetting or misinterprets coordinate spaces, a test window was positioned at a non-zero screen offset:
- Window Frame Origin: `(450.0, 320.0)`
- Window Size: `400.0 x 300.0`
- Target Text: `"Press"` (rendered on a child `BUTTON` control)

#### Empirical Coordinates:
| Parameter | Value | Interpretation |
| :--- | :--- | :--- |
| Window Frame Origin | `(450.0, 320.0)` | Top-level window screen origin |
| OCR Detected Text Box | `origin: (526.0, 389.0), size: 45.0 x 13.0` | Absolute screen bounding box |
| Calculated Action Point | `(548.5, 395.5)` | Geometric center in screen space |
| Reported `point` | `(548.5, 395.5)` | **Screen-space** (`ScreenPoint`) |
| Reported `window_point` | `(98.5, 75.5)` | **Window-local space** (`WindowPoint`) |
| Click Delivered To | `(98.5, 75.5)` | Passed to `session.window().click` |

#### Mathematical Invariant Verification:
$$\text{point.x} = \text{window.frame.origin.x} + \text{window\_point.x} = 450.0 + 98.5 = 548.5$$
$$\text{point.y} = \text{window.frame.origin.y} + \text{window\_point.y} = 320.0 + 75.5 = 395.5$$
$$\Delta = 0.000\text{ px}$$

#### Negative Proof:
If the pre-#323 coordinate bug had occurred (the caller treating `point` as window-local and the driver adding `window.frame.origin` again):
$$\text{Calculated} = (450.0 + 548.5, 320.0 + 395.5) = (998.5, 715.5)$$
The click would have landed at `(998.5, 715.5)`, which is 148 pixels to the right and 95 pixels below the window boundaries (`[450..850, 320..620]`).
In the live trial, the button clicked immediately, the window title incremented to `clicks=4`, and the before/after frame hashes changed from `f543f13048a6836d...` to `b36857364fffbd94...`.

---

## 3. End-to-End Accuracy & Offset Analysis

### 3.1 Pipeline Geometry
The pipeline flows through three transformations:
1. **Windows Media OCR**: Captures window via WGC/GDI, runs Windows OCR engine, returns text line/word bounds relative to capture image (Screen coordinates).
2. **Action Point Derivation**:
   $$\text{action\_point} = \left(\text{bounds.origin.x} + \frac{\text{bounds.size.width}}{2.0}, \text{bounds.origin.y} + \frac{\text{bounds.size.height}}{2.0}\right)$$
3. **Window to Client Transformation**:
   The driver maps `window_point` to client coordinates:
   $$\text{client\_point} = \text{window\_point} - \text{non\_client\_insets}$$
   and resolves the child control HWND via `ChildWindowFromPointEx(hwnd, client_pt, CWP_SKIPINVISIBLE | CWP_SKIPDISABLED)`.

### 3.2 Measured Offsets
- Button client rectangle: `(20.0, 20.0)` to `(160.0, 68.0)` (Width: 140px, Height: 48px).
- Button client center: `(90.0, 44.0)`.
- Non-client insets (Title bar + Window borders): Left: 8.0px, Top: 31.0px.
- Actual click landed at client coordinates:
  $$\text{client.x} = 98.5 - 8.0 = 90.5\text{ px}$$
  $$\text{client.y} = 75.5 - 31.0 = 44.5\text{ px}$$
- Offset from theoretical button center:
  $$\Delta x = +0.5\text{ px}, \quad \Delta y = +0.5\text{ px}$$
- **Conclusion**: Systematic offset is **0.0 px**. The 0.5px fractional deviation corresponds exactly to integer rounding of odd text bounding box dimensions (`45.0 x 13.0`). The click lands dead center on the text and safely within the control hit-test region.

---

## 4. Key Technical Questions Answered

### Question 1: Does the OCR → coordinates → click pipeline preserve accuracy end-to-end?
**Answer: Yes, with sub-pixel alignment.**
- Windows.Media.Ocr bounding boxes accurately delineate recognized text glyphs.
- Taking the box center generates a hit point that falls directly inside the intended child control.
- In all positive trials across two separate window instances and screen coordinates, every click triggered the intended control on the first attempt without misclicks.

### Question 2: For `window.clickText`: does the driver correctly convert to screen space, or does the caller need to?
**Answer: The caller converts screen space to window space before calling the driver.**
- In `crates/auv-cli-invoke/src/commands/window.rs` (lines 485–488):
  ```rust
  let matched = selected_window_text_match(&matches, &query, index)?;
  let point = auv_driver::geometry::ScreenPoint::from(matched.action_point());
  let window_point = session.window().to_window_point(&window, point)?;
  let action = session.window().click(&window, window_point, options.clone())?;
  ```
- And in `crates/auv-cli-invoke/src/runner.rs` (lines 377–381):
  ```rust
  let matched = crate::commands::window::selected_window_text_match(&matches, &query, selected_index)?;
  let window_point = matched_window_point(&resolved_window, matched)?;
  let response = resolved.click(window_point, options.clone()).await?;
  ```
- The driver's `session.window().click(&window, point, ...)` method takes a **`WindowPoint`** (window-frame relative). The driver then converts `WindowPoint` to client coordinates for `ChildWindowFromPointEx` and `PostMessageW(WM_LBUTTONDOWN/UP)`.
- The caller receives the OCR `action_point` (in screen coordinates), derives `window_point = session.window().to_window_point(&window, screen_point)`, and passes `window_point` to `click`. The final `WindowTextClick` struct then reports both `point` (`ScreenPoint`) and `window_point` (`WindowPoint`).

### Question 3: Any app that eats the click message but shows no effect?
**Answer: Yes, Chromium and Electron applications.**
- Standard Win32 applications (`BUTTON`, `STATIC`, `EDIT`, custom Win32 dialogs) process `PostMessageW(WM_LBUTTONDOWN)` and `PostMessageW(WM_LBUTTONUP)` in the background without needing keyboard focus or foreground activation.
- However, Chromium-based applications (such as Microsoft Edge, Chrome, VS Code, Slack) use `HWNDMessageHandler` which drops synthesized background mouse button messages unless the window has focus (`::GetFocus() == hwnd`). In Trial 8, `PostMessageW` succeeded (`delivery_result: success`), but the application exhibited **0% semantic effect** (`before_hash == after_hash`).

---

## 5. Per-Command × App-Type Evaluation Matrix

| Trial | Command | Target App | Policy | Delivery Path | Delivery Result | Semantic Effect Observed | Verdict |
| :---: | :--- | :--- | :--- | :--- | :---: | :--- | :---: |
| 1 | `window.clickText` | Win32 Anchor 1 (450, 320) | `BackgroundOnly` | `WindowTargetedMouse` | Success | Counter 0→1, label "clicks 1", hash changed | **Pass** |
| 2 | `window.clickText` | Win32 Anchor 1 (450, 320) | `BackgroundOnly` | `WindowTargetedMouse` | Success | Counter 1→2, label "clicks 2", hash changed | **Pass** |
| 3 | `window.clickText` | Win32 Anchor 1 (450, 320) | `BackgroundOnly` | `WindowTargetedMouse` | Success | Counter 2→3, label "clicks 3", hash changed | **Pass** |
| 4 | `window.clickText` | Win32 Anchor 1 (Offset Test) | `BackgroundOnly` | `WindowTargetedMouse` | Success | Counter 3→4, PR #323 screen coords verified | **Pass** |
| 5 | `window.clickText` | Win32 Anchor 1 (Negative) | `BackgroundOnly` | None | Aborted | `text_not_found`, 0 clicks sent, counter stays 4 | **Pass** |
| 6 | `window.clickText` | Win32 Surface 2 (820, 380) | `BackgroundOnly` | `WindowTargetedMouse` | Success | Counter 0→1, label "clicks 1", hash changed | **Pass** |
| 7 | `window.clickText` | Win32 Surface 2 (Negative) | `BackgroundOnly` | None | Aborted | `text_not_found`, 0 clicks sent, counter stays 1 | **Pass** |
| 8 | `window.clickText` | Microsoft Edge (Chromium) | `BackgroundOnly` | `WindowTargetedMouse` | Success | None (0% effect; Chromium dropped input) | **Expected Boundary** |
| 9 | `screen.clickText` | Global Display (Positive #1)| `BackgroundOnly` | None | Rejected | Cannot deliver without HWND; SendInput forbidden | **Unsupported** |
| 10| `screen.clickText` | Global Display (Positive #2)| `BackgroundOnly` | None | Rejected | Cannot deliver without HWND; SendInput forbidden | **Unsupported** |
| 11| `screen.clickText` | Global Display (Positive #3)| `BackgroundOnly` | None | Rejected | Cannot deliver without HWND; SendInput forbidden | **Unsupported** |
| 12| `screen.clickText` | Global Display (Negative) | `BackgroundOnly` | None | Aborted | `text_not_found`, clean abort, 0 clicks sent | **Pass (Clean Error)** |

---

## 6. Proposed Support Declarations

*Notice: In accordance with project policy, these are proposed declarations for owner review and PR scoping. No code changes to `crates/auv-driver-windows` or remote issue updates have been executed.*

| Command | Proposed Support Declaration | Documented Boundary Note |
| :--- | :--- | :--- |
| `window.clickText` | **Partial** (or **Supported** for Win32) | **Boundary**: Operates reliably under `BackgroundOnly` for standard Win32 windows and child controls via `ChildWindowFromPointEx` + `PostMessageW`. Fully conforms to PR #323 screen-space `point` and window-local `window_point` semantics. Unsupported under `BackgroundOnly` on Chromium/Electron apps (which require foreground window focus to process mouse button events). Requires `--target` resolution. |
| `screen.clickText` | **Unsupported** under `BackgroundOnly` | **Boundary**: Inherently incompatible with `InputPolicy::BackgroundOnly` because display-level clicking lacks a target window for `PostMessageW`. Screen clicks must use `SendInput`, which requires foreground window activation and moves the physical system cursor. |

### Summary for Issue #211
- `window.clickText`: Mark as **Partial** on Windows under `BackgroundOnly`, noting that Win32 UI controls work end-to-end with PR #323 screen coordinates, but Chromium/Electron windows discard background mouse clicks.
- `screen.clickText`: Keep as **Unsupported** on Windows under `BackgroundOnly` (cannot fake background delivery without a target window).
