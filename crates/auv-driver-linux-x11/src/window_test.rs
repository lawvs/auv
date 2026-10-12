use super::*;
use auv_driver_common::App;

fn facts(class: &[u8]) -> WindowFacts {
  WindowFacts {
    wm_class: Some(class.to_vec()),
    normal_type: true,
    viewable: true,
    client_origin: (10, 40),
    client_size: (300, 200),
    ..WindowFacts::default()
  }
}

fn client(xid: u32, class: &[u8], pid: Option<u32>, title: &str) -> ClientWindow {
  client_window(
    xid,
    WindowFacts {
      net_wm_name: Some(title.as_bytes().to_vec()),
      process_id: pid,
      ..facts(class)
    },
  )
}

#[test]
fn window_ids_round_trip_and_reject_foreign_or_zero_ids() {
  assert_eq!(window_id(0x1a00007), "x11:0x1a00007");
  assert_eq!(parse_window_id("x11:0x1a00007").unwrap(), 0x1a00007);
  for id in [
    "0x1a00007",
    "x11:0x0",
    "x11:0x",
    "x11:0x+12",
    "x11:0xzz",
    "x11:0x100000000",
    "123",
  ] {
    assert!(matches!(parse_window_id(id), Err(DriverError::InvalidInput { .. })), "{id}");
  }
}

#[test]
fn wm_class_yields_instance_and_class() {
  assert_eq!(
    parse_wm_class(b"navigator\0Firefox\0"),
    Some(WmClass {
      instance: "navigator".into(),
      class: "Firefox".into(),
    })
  );
  // A missing trailing NUL or class is tolerated; an empty property is not.
  assert_eq!(parse_wm_class(b"xterm").map(|class| class.class), Some(String::new()));
  assert_eq!(parse_wm_class(b"\0\0"), None);
}

#[test]
fn title_prefers_utf8_net_wm_name_then_latin1_wm_name() {
  assert_eq!(decode_title(Some("Ωmega".as_bytes()), Some(b"legacy")), Some("Ωmega".into()));
  assert_eq!(decode_title(None, Some(b"caf\xe9")), Some("café".into()));
  // Invalid UTF-8 in _NET_WM_NAME falls back to WM_NAME rather than inventing text.
  assert_eq!(decode_title(Some(b"\xff\xfe"), Some(b"plain")), Some("plain".into()));
  assert_eq!(decode_title(Some(b""), None), None);
}

#[test]
fn frame_includes_window_manager_decorations() {
  assert_eq!(outer_frame((10, 40), (300, 200), Some([2, 3, 30, 4])), Rect::new(8.0, 10.0, 305.0, 234.0));
  assert_eq!(outer_frame((10, 40), (300, 200), None), Rect::new(10.0, 40.0, 300.0, 200.0));
}

#[test]
fn application_id_prefers_gtk_application_id_then_wm_class() {
  let gtk = client_window(
    1,
    WindowFacts {
      gtk_application_id: Some(b"org.gnome.TextEditor".to_vec()),
      ..facts(b"gnome-text-editor\0Gnome-text-editor\0")
    },
  );
  assert_eq!(gtk.window.application_id.as_deref(), Some("org.gnome.TextEditor"));
  assert_eq!(gtk.window.app_name.as_deref(), Some("Gnome-text-editor"));

  let plain = client(2, b"navigator\0Firefox\0", Some(7), "Mozilla Firefox");
  assert_eq!(plain.window.application_id.as_deref(), Some("Firefox"));
  assert_eq!(plain.window.reference.id, "x11:0x2");
  assert_eq!(plain.window.process_id, Some(7));
  assert_eq!(plain.window.coordinate_space, CoordinateSpace::Screen);

  // Application targets match every identity case-insensitively.
  for id in ["firefox", "FIREFOX", "Navigator"] {
    assert!(plain.is_application(&TextMatcher::Exact(id.into())), "{id}");
  }
  assert!(gtk.is_application(&TextMatcher::Exact("org.gnome.texteditor".into())));
  assert!(gtk.is_application(&TextMatcher::Exact("gnome-text-editor".into())));
  assert!(!plain.is_application(&TextMatcher::Exact("fire".into())));
  assert!(plain.is_application(&TextMatcher::Contains("FIRE".into())));
}

#[test]
fn visibility_and_main_flags_follow_map_state_and_ewmh_hints() {
  let hidden = client_window(
    1,
    WindowFacts {
      hidden: true,
      ..facts(b"a\0A\0")
    },
  );
  assert!(!hidden.window.is_visible);
  let unmapped = client_window(
    2,
    WindowFacts {
      viewable: false,
      ..facts(b"a\0A\0")
    },
  );
  assert!(!unmapped.window.is_visible);
  let dialog = client_window(
    3,
    WindowFacts {
      transient: true,
      ..facts(b"a\0A\0")
    },
  );
  assert!(dialog.window.is_visible && !dialog.window.is_main);
}

#[test]
fn selectors_match_app_identity_title_and_frontmost() {
  let clients = vec![
    client(3, b"editor\0Editor\0", Some(30), "notes.txt — Editor"),
    client(2, b"editor\0Editor\0", Some(30), "todo.txt — Editor"),
    client(1, b"navigator\0Firefox\0", Some(10), "Mozilla Firefox"),
  ];

  let firefox = resolve_from(&clients, &WindowSelector::default().owned_by(App::id("firefox")), None).unwrap();
  assert_eq!(firefox.xid, 1);
  let todo = resolve_from(&clients, &WindowSelector::default().owned_by(App::id("editor")).title_contains("TODO"), None).unwrap();
  assert_eq!(todo.xid, 2);
  assert!(matches!(
    resolve_from(&clients, &WindowSelector::default().owned_by(App::id("editor")), None),
    Err(DriverError::InvalidInput { .. })
  ));
  assert!(matches!(
    resolve_from(&clients, &WindowSelector::default().owned_by(App::id("missing")), None),
    Err(DriverError::NotFound { .. })
  ));

  // main_visible keeps top-most stacking order unless the active window ranks first.
  let main = WindowSelector {
    main_visible: true,
    ..WindowSelector::default().owned_by(App::id("editor"))
  };
  assert_eq!(resolve_from(&clients, &main, None).unwrap().xid, 3);
  assert_eq!(resolve_from(&clients, &main, Some(2)).unwrap().xid, 2);

  let frontmost = WindowSelector {
    main_visible: true,
    ..WindowSelector::default().owned_by(App::frontmost())
  };
  assert_eq!(resolve_from(&clients, &frontmost, Some(1)).unwrap().xid, 1);
  assert!(resolve_from(&clients, &frontmost, None).is_err());
}

#[test]
fn application_target_prefers_active_window_and_rejects_multiple_instances() {
  let clients = vec![
    client(3, b"editor\0Editor\0", Some(30), "a"),
    client(2, b"editor\0Editor\0", Some(30), "b"),
  ];
  assert_eq!(application_window(&clients, "Editor", None).unwrap().xid, 3);
  assert_eq!(application_window(&clients, "editor", Some(2)).unwrap().xid, 2);
  assert!(matches!(application_window(&clients, "missing", None), Err(DriverError::NotFound { .. })));

  let two_processes = vec![
    client(3, b"editor\0Editor\0", Some(30), "a"),
    client(2, b"editor\0Editor\0", Some(31), "b"),
  ];
  assert!(matches!(application_window(&two_processes, "editor", Some(2)), Err(DriverError::InvalidInput { .. })));

  // Without _NET_WM_PID, separate windows cannot be proven to share a process.
  let unknown = vec![
    client(3, b"editor\0Editor\0", None, "a"),
    client(2, b"editor\0Editor\0", None, "b"),
  ];
  assert!(matches!(application_window(&unknown, "editor", None), Err(DriverError::InvalidInput { .. })));
  assert_eq!(application_window(&unknown[..1], "editor", None).unwrap().xid, 3);
}
