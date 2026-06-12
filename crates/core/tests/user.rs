use rind_core::prelude::ToUstr;
use rind_core::user::{
  PamConfig, PamError, PamHandle, ShadowEntry, UserStore, UserStoreShared, verify_password,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn temp_dir() -> std::path::PathBuf {
  let dir = std::env::temp_dir().join(format!(
    "rind-user-test-{}-{}",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos()
  ));
  fs::create_dir_all(&dir).unwrap();
  dir
}

fn write_file(dir: &Path, name: &str, content: &str) {
  let path = dir.join(name);
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).unwrap();
  }
  let mut f = fs::File::create(&path).unwrap();
  f.write_all(content.as_bytes()).unwrap();
}

fn generate_test_hash(password: &str) -> String {
  let params = sha_crypt::Sha512Params::new(5000).expect("valid rounds");
  sha_crypt::sha512_simple(password, &params).expect("hash generation")
}

fn make_test_store(dir: &Path) -> UserStoreShared {
  let hash = generate_test_hash("test123");

  write_file(
    dir,
    "etc/passwd",
    "root:x:0:0:root:/root:/bin/sh\nmakano:x:1000:1000:Makano:/home/makano:/bin/sh\nlocked:x:1001:1001:Locked:/home/locked:/bin/sh\n",
  );
  write_file(
    dir,
    "etc/shadow",
    &format!(
      "root:{hash}:19000:0:99999:7:::\nmakano:{hash}:19000:0:99999:7:::\nlocked:!:19000:0:99999:7:::\n"
    ),
  );
  write_file(
    dir,
    "etc/group",
    "root:x:0:root\nwheel:x:10:makano\nmakano:x:1000:\nlocked:x:1001:\n",
  );

  Arc::new(UserStore::load_from_root(dir).expect("should load test store"))
}

#[test]
fn user_store_loads_and_lookups() {
  let dir = temp_dir();
  let store = make_test_store(&dir);

  assert!(store.lookup_by_name("root").is_some());
  assert!(store.lookup_by_name("makano").is_some());
  assert!(store.lookup_by_name("nobody").is_none());

  let makano = store.lookup_by_name("makano").unwrap();
  assert_eq!(makano.uid, 1000);
  assert_eq!(makano.gid, 1000);
  assert_eq!(makano.home, "/home/makano");

  assert!(store.lookup_by_uid(0).is_some());
  assert!(store.lookup_by_uid(1000).is_some());
  assert!(store.lookup_by_uid(9999).is_none());

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn user_groups() {
  let dir = temp_dir();
  let store = make_test_store(&dir);

  let makano = store.lookup_by_name("makano").unwrap();
  let groups = store.groups_for(makano);
  assert!(groups.contains(&"makano".to_ustr()));
  assert!(groups.contains(&"wheel".to_ustr()));
  assert!(store.user_in_group(makano, "wheel"));
  assert!(!store.user_in_group(makano, "root"));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn password_verification_works() {
  let hash = generate_test_hash("hello");
  assert!(verify_password("hello", &hash));
  assert!(!verify_password("wrong", &hash));
  assert!(!verify_password("hello", "!"));
  assert!(!verify_password("hello", "*"));
  assert!(!verify_password("hello", ""));
}

#[test]
fn pam_authenticate_success() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let user = pam.pam_authenticate("makano", "test123").unwrap();
  assert_eq!(user.username.as_str(), "makano");

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_authenticate_wrong_password() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let result = pam.pam_authenticate("makano", "wrong");
  assert!(matches!(result, Err(PamError::AuthenticationFailed)));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_authenticate_locked_account() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let result = pam.pam_authenticate("locked", "test123");
  assert!(matches!(result, Err(PamError::AccountLocked)));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_authenticate_user_not_found() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let result = pam.pam_authenticate("nobody", "test123");
  assert!(matches!(result, Err(PamError::UserNotFound)));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_lockout_after_max_retries() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::with_config(
    store,
    PamConfig {
      max_retries: 3,
      lockout_duration: Duration::from_secs(60),
    },
  );

  for _ in 0..3 {
    let _ = pam.pam_authenticate("makano", "wrong");
  }

  let result = pam.pam_authenticate("makano", "test123");
  assert!(matches!(result, Err(PamError::MaxRetriesExceeded)));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_auto_login() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let user = pam.pam_authenticate_auto("makano").unwrap();
  assert_eq!(user.username.as_str(), "makano");

  let result = pam.pam_authenticate_auto("locked");
  assert!(matches!(result, Err(PamError::AccountLocked)));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn pam_session_lifecycle() {
  let dir = temp_dir();
  let store = make_test_store(&dir);
  let pam = PamHandle::new(store);

  let session = pam.pam_open_session("makano", "/dev/tty1").unwrap();
  assert_eq!(session.username, "makano");
  assert!(pam.has_active_session("makano"));
  assert_eq!(pam.sessions_for("makano").len(), 1);

  pam.pam_close_session(session.id).unwrap();
  assert!(!pam.has_active_session("makano"));
  assert_eq!(pam.sessions_for("makano").len(), 0);

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn shadow_entry_expiry_checks() {
  let entry = ShadowEntry {
    username: "test".to_ustr(),
    password_hash: "$6$salt$hash".to_string(),
    last_changed: Some(1),
    min_days: Some(0),
    max_days: Some(1),
    warn_days: Some(7),
    inactive_days: None,
    expire_date: Some(1),
  };
  assert!(entry.is_expired());
  assert!(entry.password_expired());
  assert!(!entry.is_locked());
}

#[test]
fn write_perms_with_overlay_persists_effective_permissions() {
  let dir = temp_dir();
  let store = make_test_store(&dir);

  let mut user_grants: HashMap<u32, HashSet<u16>> = HashMap::new();
  let mut user_revokes: HashMap<u32, HashSet<u16>> = HashMap::new();
  let mut group_grants: HashMap<u32, HashSet<u16>> = HashMap::new();
  let group_revokes: HashMap<u32, HashSet<u16>> = HashMap::new();

  user_grants.entry(1000).or_default().insert(42);
  user_revokes.entry(1000).or_default().insert(999);
  group_grants.entry(10).or_default().insert(77);

  store
    .write_perms_with_overlay(
      &dir.join("etc/rperms"),
      &user_grants,
      &user_revokes,
      &group_grants,
      &group_revokes,
    )
    .expect("should write perms");

  let reloaded = UserStore::load_from_root(&dir).expect("should reload store");
  let user = reloaded.lookup_by_uid(1000).expect("user exists");
  let wheel = reloaded.group(10).expect("group exists");

  assert!(user.permissions.contains(&42));
  assert!(wheel.permissions.contains(&77));

  let _ = fs::remove_dir_all(dir);
}

#[test]
fn write_perms_revoking_absent_permission_is_noop() {
  let dir = temp_dir();
  let store = make_test_store(&dir);

  let user_grants: HashMap<u32, HashSet<u16>> = HashMap::new();
  let mut user_revokes: HashMap<u32, HashSet<u16>> = HashMap::new();
  let group_grants: HashMap<u32, HashSet<u16>> = HashMap::new();
  let group_revokes: HashMap<u32, HashSet<u16>> = HashMap::new();

  user_revokes.entry(1000).or_default().insert(999);

  store
    .write_perms_with_overlay(
      &dir.join("etc/rperms"),
      &user_grants,
      &user_revokes,
      &group_grants,
      &group_revokes,
    )
    .expect("should write perms");

  let reloaded = UserStore::load_from_root(&dir).expect("should reload store");
  let user = reloaded.lookup_by_uid(1000).expect("user exists");
  assert!(
    user.permissions.is_empty(),
    "revoking absent perm should produce no permissions entry"
  );

  let _ = fs::remove_dir_all(dir);
}
