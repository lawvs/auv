//! Real X11 integration probe. Run only in an isolated 800x600 Xvfb session.
#![cfg(target_os = "linux")]

use auv_driver_common::{
  CaptureOptions, Click, ClickModifiers, DisturbanceLevel, Driver, InputDeliveryPath, InputPolicy, InputTarget, KeyPressOptions,
  MoveMouseRequest, PasteTextOptions, Point, Rect, Scroll, TextSubmit, TypeTextOptions, input::MouseButton,
};
use auv_driver_linux_x11::X11Driver;
use std::{
  fs,
  io::Write,
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  sync::Mutex,
  time::{Duration, Instant},
};
use x11rb::{
  connection::Connection,
  protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass},
};

// Both tests drive the same display; run them one at a time.
static DESKTOP: Mutex<()> = Mutex::new(());

struct Fixture {
  child: Child,
  directory: PathBuf,
}
impl Drop for Fixture {
  fn drop(&mut self) {
    let _ = self.child.kill();
    let _ = self.child.wait();
    let _ = fs::remove_dir_all(&self.directory);
  }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
  let started = Instant::now();
  while !condition() {
    assert!(started.elapsed() < Duration::from_secs(10), "X11 fixture timed out");
    std::thread::sleep(Duration::from_millis(30));
  }
}

#[test]
#[ignore = "requires isolated Xvfb and python3-tk; manipulates the selected desktop"]
fn capture_and_input_change_real_application_state() {
  let _desktop = DESKTOP.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let directory = std::env::temp_dir().join(format!("auv-x11-test-{}", std::process::id()));
  fs::create_dir(&directory).unwrap();
  let child = Command::new("python3")
    .arg("-c")
    .arg(
      r#"
import pathlib, sys, tkinter as tk
p = pathlib.Path(sys.argv[1])
root = tk.Tk()
root.overrideredirect(True)
root.geometry('800x600+0+0')
root.configure(background='white')
entry = tk.Entry(root)
entry.place(x=20, y=20, width=300, height=40)
# NOTICE: Tk's Linux default Ctrl+A means beginning-of-line. This fixture
# explicitly models editors with select-all, the driver's replacement contract.
# Keep this binding while the fixture uses Tk instead of a desktop editor.
def select_all(event):
    entry.selection_range(0, tk.END)
    return 'break'
entry.bind('<Control-a>', select_all)
canvas = tk.Canvas(root, background='#ff0000', highlightthickness=0)
canvas.place(x=200, y=150, width=100, height=100)
listbox = tk.Listbox(root)
listbox.place(x=400, y=300, width=200, height=200)
for i in range(100):
    listbox.insert(tk.END, 'item '+str(i))
def record_scroll(event):
    root.after(50, lambda: (p/'scroll_position').write_text(str(listbox.yview()[0])))
listbox.bind('<Button-5>', record_scroll, add='+')
def submit(event):
    (p/'text').write_text(entry.get())
    canvas.configure(background='#0000ff')
entry.bind('<Return>', submit)
def event(e):
    with (p/'events').open('a') as f:
        f.write(str(e.num)+':'+str(e.state & 1)+'\n')
root.bind('<ButtonPress>', event)
def transition(kind):
    def record(e):
        with (p/'transitions').open('a') as f:
            f.write(kind+':'+str(e.keysym if kind.startswith('key') else e.num)+'\n')
    return record
root.bind('<ButtonPress>', transition('button-down'), add='+')
root.bind('<ButtonRelease>', transition('button-up'))
root.bind('<KeyPress>', transition('key-down'))
root.bind('<KeyRelease>', transition('key-up'))
root.update()
(p/'ready').touch()
root.mainloop()
"#,
    )
    .arg(&directory)
    .spawn()
    .unwrap();
  let fixture = Fixture { child, directory };
  wait_for(|| fixture.directory.join("ready").exists());
  let session = X11Driver.open_local().unwrap();
  let display = session.display().capture(CaptureOptions::default()).unwrap();
  assert_eq!((display.capture.image.width(), display.capture.image.height()), (800, 600));
  assert_eq!(display.capture.image.get_pixel(220, 170).0, [255, 0, 0, 255]);
  let region = session
    .display()
    .capture_region(CaptureOptions {
      region: Some(Rect::new(210.0, 160.0, 20.0, 20.0)),
      ..Default::default()
    })
    .unwrap();
  assert_eq!(region.capture.image.get_pixel(5, 5).0, [255, 0, 0, 255]);
  assert_eq!(region.capture.bounds.origin, Point::new(210.0, 160.0));
  assert!(
    session
      .display()
      .capture_region(CaptureOptions {
        region: Some(Rect::new(790.0, 590.0, 20.0, 20.0)),
        ..Default::default()
      })
      .is_err()
  );

  let input = session.input();
  input.click_at(Point::new(50.0, 40.0), Click::Single, ClickModifiers::default()).unwrap();
  input.type_text("discard", TypeTextOptions::default()).unwrap();
  input
    .type_text(
      "AUV Ω",
      TypeTextOptions {
        replace_existing: true,
        submit: TextSubmit::Return,
        ..Default::default()
      },
    )
    .unwrap();
  wait_for(|| fixture.directory.join("text").exists());
  assert_eq!(fs::read_to_string(fixture.directory.join("text")).unwrap(), "AUV Ω");
  wait_for(|| session.display().capture(CaptureOptions::default()).unwrap().capture.image.get_pixel(220, 170).0 == [0, 0, 255, 255]);
  input.click_button_at(Point::new(450.0, 450.0), MouseButton::Right, Click::Single, ClickModifiers::default()).unwrap();
  input.scroll_at(Point::new(450.0, 450.0), Scroll::new(120.0, 120.0), Duration::ZERO).unwrap();
  wait_for(|| {
    fs::read_to_string(fixture.directory.join("scroll_position"))
      .ok()
      .and_then(|position| position.parse::<f64>().ok())
      .is_some_and(|position| position > 0.0)
  });
  input.drag(Point::new(450.0, 450.0), Point::new(500.0, 500.0), MouseButton::Left).unwrap();
  assert_eq!(input.current_position().unwrap(), Point::new(500.0, 500.0));
  let (point, _) = input.move_mouse(MoveMouseRequest::direct(Point::new(520.0, 520.0)), |_| true).unwrap();
  assert_eq!(point, Point::new(520.0, 520.0));
  let (point, _) = input.drag_mouse(MoveMouseRequest::direct(Point::new(540.0, 540.0)), MouseButton::Left).unwrap();
  assert_eq!(point, Point::new(540.0, 540.0));

  let mouse = input.create_mouse().unwrap();
  input.mouse_down(&InputTarget::Foreground, mouse, Point::new(560.0, 560.0), MouseButton::Left, Duration::from_secs(1)).unwrap();
  input.move_mouse_to(mouse, Point::new(580.0, 580.0)).unwrap();
  input.mouse_up(mouse).unwrap();
  input.remove_mouse(mouse).unwrap();

  input.click_at(Point::new(50.0, 40.0), Click::Single, ClickModifiers::default()).unwrap();
  let hold =
    input.key_down(&InputTarget::Foreground, vec!["Shift".into()], InputPolicy::ForegroundPreferred, Duration::from_secs(1)).unwrap();
  input.key_up(hold.into_id()).unwrap();
  let control = input
    .key_down(&InputTarget::Foreground, vec!["ctrl".into()], InputPolicy::ForegroundPreferred, Duration::from_secs(2))
    .unwrap()
    .into_id();
  let shift = input
    .key_down(&InputTarget::Foreground, vec!["shift".into()], InputPolicy::ForegroundPreferred, Duration::from_secs(2))
    .unwrap()
    .into_id();
  // Both names resolve to the same X keysym; a second press must be refused
  // before delivery rather than releasing the first owner's modifier.
  assert!(
    input.key_down(&InputTarget::Foreground, vec!["control".into()], InputPolicy::ForegroundPreferred, Duration::from_secs(2)).is_err()
  );
  input
    .press_key(KeyPressOptions {
      key: "p".into(),
      ..Default::default()
    })
    .unwrap();
  input.key_up(shift).unwrap();
  input.key_up(control).unwrap();
  input
    .press_key(KeyPressOptions {
      key: "Escape".into(),
      ..Default::default()
    })
    .unwrap();
  // NOTICE: Tk 8.6 translates X11 button 7 to button 5 + Shift. Assert the
  // toolkit-level observation until this fixture uses raw X11 events instead.
  // Source: https://github.com/tcltk/tk/blob/core-8-6-13/generic/tkEvent.c#L1135-L1140
  wait_for(|| {
    fs::read_to_string(fixture.directory.join("events"))
      .is_ok_and(|events| events.contains("3:0\n") && events.contains("5:0\n") && events.contains("5:1\n"))
  });
  wait_for(|| {
    fs::read_to_string(fixture.directory.join("transitions")).is_ok_and(|events| {
      events.contains("button-down:1\n")
        && events.contains("button-up:1\n")
        && events.contains("key-down:Shift_L\n")
        && events.contains("key-up:Shift_L\n")
        && events.contains("key-down:Control_L\nkey-down:Shift_L\n")
        && events.contains("key-up:Shift_L\nkey-up:Control_L\n")
    })
  });
}

fn spawn_paste_fixture(directory: &Path) -> Child {
  Command::new("python3")
    .arg("-c")
    .arg(
      r#"
import pathlib, sys, tkinter as tk
p = pathlib.Path(sys.argv[1])
root = tk.Tk()
root.overrideredirect(True)
root.geometry('800x600+0+0')
entry = tk.Entry(root)
entry.place(x=20, y=20, width=300, height=40)
def submit(event):
    (p/'text.tmp').write_text(entry.get())
    (p/'text.tmp').rename(p/'text')
    entry.delete(0, tk.END)
entry.bind('<Return>', submit)
def dump_clipboard(event):
    try:
        value = root.clipboard_get()
    except tk.TclError as error:
        value = 'ERR:' + str(error)
    (p/'clipboard.tmp').write_text(value)
    (p/'clipboard.tmp').rename(p/'clipboard')
root.bind_all('<F5>', dump_clipboard)
root.bind_all('<F6>', lambda event: root.focus_set())
root.update()
(p/'ready').touch()
root.mainloop()
"#,
    )
    .arg(directory)
    .spawn()
    .unwrap()
}

fn take_file(path: &Path) -> String {
  wait_for(|| path.exists());
  let value = fs::read_to_string(path).unwrap();
  fs::remove_file(path).unwrap();
  value
}

#[test]
#[ignore = "requires isolated Xvfb, python3-tk and xclip; manipulates the selected desktop"]
fn clipboard_paste_delivers_text_and_restores_the_previous_clipboard() {
  let _desktop = DESKTOP.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let directory = std::env::temp_dir().join(format!("auv-x11-paste-test-{}", std::process::id()));
  fs::create_dir(&directory).unwrap();
  let child = spawn_paste_fixture(&directory);
  let fixture = Fixture { child, directory };
  wait_for(|| fixture.directory.join("ready").exists());

  // An independent client observes CLIPBOARD ownership.
  let (observer, screen) = x11rb::connect(None).unwrap();
  let root = observer.setup().roots[screen].root;
  let clipboard = observer.intern_atom(false, b"CLIPBOARD").unwrap().reply().unwrap().atom;
  let owner = || observer.get_selection_owner(clipboard).unwrap().reply().unwrap().owner;
  observer.set_selection_owner(x11rb::NONE, clipboard, x11rb::CURRENT_TIME).unwrap();
  assert_eq!(owner(), x11rb::NONE);

  let session = X11Driver.open_local().unwrap();
  let input = session.input();
  let text_file = fixture.directory.join("text");
  let clipboard_file = fixture.directory.join("clipboard");
  let submit = |text: &str| PasteTextOptions {
    text: text.into(),
    submit: TextSubmit::Return,
    ..Default::default()
  };
  input.click_at(Point::new(50.0, 40.0), Click::Single, ClickModifiers::default()).unwrap();

  // 1. Empty clipboard: the paste lands and CLIPBOARD is released again.
  let result = input.paste_text(submit("héllo 世界")).unwrap();
  assert_eq!(take_file(&text_file), "héllo 世界");
  assert_eq!(result.selected_path, InputDeliveryPath::ClipboardPaste);
  assert_eq!(result.clipboard_disturbance, DisturbanceLevel::Temporary);
  assert!(!result.verified);
  assert_eq!(owner(), x11rb::NONE);

  // 2. Text owned by another client: the paste lands and an independent
  // reader gets the previous text back afterwards.
  let mut xclip = Command::new("xclip").args(["-selection", "clipboard", "-quiet"]).stdin(Stdio::piped()).spawn().unwrap();
  xclip.stdin.take().unwrap().write_all("before ü".as_bytes()).unwrap();
  wait_for(|| owner() != x11rb::NONE);
  let result = input.paste_text(submit("AUV Ω")).unwrap();
  assert_eq!(take_file(&text_file), "AUV Ω");
  assert_eq!(result.clipboard_disturbance, DisturbanceLevel::Temporary, "{result:?}");
  // xclip exits after losing CLIPBOARD to the paste.
  let _ = xclip.wait();
  input
    .press_key(KeyPressOptions {
      key: "F5".into(),
      ..Default::default()
    })
    .unwrap();
  assert_eq!(take_file(&clipboard_file), "before ü");
  assert_eq!(session.clipboard().snapshot().unwrap(), "before ü");

  // 3. Another client takes CLIPBOARD during the settle period: it keeps it.
  let thief = observer.generate_id().unwrap();
  observer
    .create_window(0, thief, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new())
    .unwrap()
    .check()
    .unwrap();
  let result = std::thread::scope(|scope| {
    let paste = scope.spawn(|| {
      input.paste_text(PasteTextOptions {
        settle: Duration::from_millis(1500),
        ..submit("stolen")
      })
    });
    std::thread::sleep(Duration::from_millis(700));
    observer.set_selection_owner(thief, clipboard, x11rb::CURRENT_TIME).unwrap();
    observer.flush().unwrap();
    paste.join().unwrap()
  })
  .unwrap();
  assert_eq!(take_file(&text_file), "stolen");
  assert_eq!(result.clipboard_disturbance, DisturbanceLevel::Unknown);
  assert!(result.attempts[0].message.as_deref().is_some_and(|message| message.contains("another client")), "{result:?}");
  assert_eq!(owner(), thief);

  // 4. The thief never answers conversions: paste fails before any key.
  let error = input.paste_text(submit("never")).unwrap_err();
  assert!(error.to_string().contains("did not answer"), "{error}");
  assert_eq!(owner(), thief);
  assert!(!text_file.exists());

  // 5. Nothing requests the text: the paste fails and CLIPBOARD is empty
  // again, as it was before the paste.
  observer.set_selection_owner(x11rb::NONE, clipboard, x11rb::CURRENT_TIME).unwrap();
  observer.flush().unwrap();
  input
    .press_key(KeyPressOptions {
      key: "F6".into(),
      ..Default::default()
    })
    .unwrap();
  let error = input.paste_text(submit("ignored")).unwrap_err();
  assert!(error.to_string().contains("no paste consumer"), "{error}");
  assert_eq!(owner(), x11rb::NONE);
}
