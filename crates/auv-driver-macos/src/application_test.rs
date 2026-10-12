use super::*;

#[test]
fn activation_script_is_scoped_to_exact_bundle_id() {
  assert_eq!(activation_script("com.apple.TextEdit").expect("script"), "tell application id \"com.apple.TextEdit\" to activate");
}

#[test]
fn activation_script_rejects_blank_bundle_id() {
  let error = activation_script("   ").expect_err("blank bundle id should fail");
  assert!(error.to_string().contains("non-empty bundle id"));
}

#[test]
fn activation_verification_distinguishes_verified_foreground_from_activation_only() {
  assert_eq!(
    activation_verification("com.apple.TextEdit", Ok("com.apple.TextEdit".to_string())),
    ApplicationActivationVerification::VerifiedForeground {
      observed_application_id: "com.apple.TextEdit".to_string(),
    }
  );
  assert_eq!(
    activation_verification("com.apple.TextEdit", Ok("com.apple.finder".to_string())),
    ApplicationActivationVerification::ForegroundMismatch {
      observed_application_id: "com.apple.finder".to_string(),
    }
  );
  assert_eq!(
    activation_verification("com.apple.TextEdit", Err("observation denied".to_string())),
    ApplicationActivationVerification::Unavailable {
      reason: "observation denied".to_string(),
    }
  );
}

#[test]
fn activation_verification_reports_unavailable_when_no_frontmost_bundle_id_was_observed() {
  assert_eq!(
    activation_verification("com.apple.TextEdit", Ok("  ".to_string())),
    ApplicationActivationVerification::Unavailable {
      reason: "WindowServer observation did not identify a frontmost application bundle id".to_string(),
    }
  );
}

#[test]
fn activation_verification_matches_bundle_ids_case_insensitively() {
  assert_eq!(
    activation_verification("com.apple.textedit", Ok("com.apple.TextEdit".to_string())),
    ApplicationActivationVerification::VerifiedForeground {
      observed_application_id: "com.apple.TextEdit".to_string(),
    }
  );
}

#[test]
fn activation_script_escapes_applescript_string_content() {
  assert_eq!(activation_script("com.example.\\\"quoted").expect("script"), "tell application id \"com.example.\\\\\\\"quoted\" to activate");
}
