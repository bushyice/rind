use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct UserRecord {
  pub username: String,
  pub uid: u32,
  pub gid: u32,
  pub gecos: String,
  pub home: String,
  pub shell: String,
  pub permissions: Vec<u16>,
}

impl UserRecord {
  fn from_passwd_line(line: &str) -> Option<Self> {
    let parts: Vec<&str> = line.splitn(7, ':').collect();
    if parts.len() < 7 {
      return None;
    }
    Some(Self {
      username: parts[0].to_string(),
      uid: parts[2].parse().ok()?,
      gid: parts[3].parse().ok()?,
      gecos: parts[4].to_string(),
      home: parts[5].to_string(),
      shell: parts[6].to_string(),
      permissions: Vec::new(),
    })
  }

  pub fn is_system_user(&self) -> bool {
    self.uid < 1000
  }
}

#[derive(Debug, Clone)]
pub struct ShadowEntry {
  pub username: String,
  pub password_hash: String,
  pub last_changed: Option<i64>,
  pub min_days: Option<i64>,
  pub max_days: Option<i64>,
  pub warn_days: Option<i64>,
  pub inactive_days: Option<i64>,
  pub expire_date: Option<i64>,
}

impl ShadowEntry {
  fn from_shadow_line(line: &str) -> Option<Self> {
    let parts: Vec<&str> = line.splitn(9, ':').collect();
    if parts.len() < 8 {
      return None;
    }
    let parse_opt = |s: &str| -> Option<i64> { if s.is_empty() { None } else { s.parse().ok() } };
    Some(Self {
      username: parts[0].to_string(),
      password_hash: parts[1].to_string(),
      last_changed: parse_opt(parts[2]),
      min_days: parse_opt(parts[3]),
      max_days: parse_opt(parts[4]),
      warn_days: parse_opt(parts[5]),
      inactive_days: parse_opt(parts[6]),
      expire_date: parse_opt(parts[7]),
    })
  }

  pub fn is_locked(&self) -> bool {
    self.password_hash.is_empty()
      || self.password_hash.starts_with('!')
      || self.password_hash.starts_with('*')
  }

  pub fn is_expired(&self) -> bool {
    if let Some(expire) = self.expire_date {
      let today = days_since_epoch();
      expire > 0 && today >= expire
    } else {
      false
    }
  }

  pub fn password_expired(&self) -> bool {
    match (self.last_changed, self.max_days) {
      (Some(last), Some(max)) if max > 0 => {
        let today = days_since_epoch();
        today >= last + max
      }
      _ => false,
    }
  }
}

fn days_since_epoch() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| (d.as_secs() / 86400) as i64)
    .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct GroupEntry {
  pub name: String,
  pub gid: u32,
  pub members: Vec<String>,
  pub permissions: Vec<u16>,
}

impl GroupEntry {
  fn from_group_line(line: &str) -> Option<Self> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() < 4 {
      return None;
    }
    Some(Self {
      name: parts[0].to_string(),
      gid: parts[2].parse().ok()?,
      members: parts[3]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect(),
      permissions: Vec::new(),
    })
  }
}

#[derive(Debug, Clone, Default)]
pub struct UserStore {
  users: Vec<UserRecord>,
  shadows: Vec<ShadowEntry>,
  groups: Vec<GroupEntry>,
  name_index: HashMap<String, usize>,
  uid_index: HashMap<u32, usize>,
}

pub type UserStoreShared = Arc<UserStore>;

impl UserStore {
  pub fn load_system() -> Result<Self, CoreError> {
    Self::load(
      Path::new("/etc/passwd"),
      Path::new("/etc/shadow"),
      Path::new("/etc/group"),
      Path::new("/etc/rperms"),
    )
  }

  pub fn load_from_root(root: &Path) -> Result<Self, CoreError> {
    Self::load(
      &root.join("etc/passwd"),
      &root.join("etc/shadow"),
      &root.join("etc/group"),
      &root.join("etc/rperms"),
    )
  }

  pub fn load(
    passwd_path: &Path,
    shadow_path: &Path,
    group_path: &Path,
    perms_path: &Path,
  ) -> Result<Self, CoreError> {
    let mut store = Self::default();

    let mut user_perms = HashMap::new();
    let mut group_perms = HashMap::new();

    if perms_path.exists() {
      let data = fs::read(perms_path)
        .map_err(|e| CoreError::Custom(format!("failed to read {}: {e}", passwd_path.display())))?; // raw bytes
      let mut cursor = Cursor::new(data);

      while (cursor.position() as usize) < cursor.get_ref().len() {
        let mut t = [0u8; 1];
        cursor.read_exact(&mut t).map_err(CoreError::custom)?;
        let is_user = t[0] == 0;

        let mut id_buf = [0u8; 4];
        cursor.read_exact(&mut id_buf).map_err(CoreError::custom)?;
        let id = u32::from_le_bytes(id_buf);

        let mut count_buf = [0u8; 2];
        cursor
          .read_exact(&mut count_buf)
          .map_err(CoreError::custom)?;
        let count = u16::from_le_bytes(count_buf);

        let mut perms = Vec::with_capacity(count as usize);
        for _ in 0..count {
          let mut p = [0u8; 2];
          cursor.read_exact(&mut p).map_err(CoreError::custom)?;
          perms.push(u16::from_le_bytes(p));
        }

        if is_user {
          user_perms.insert(id, perms);
        } else {
          group_perms.insert(id, perms);
        }
      }
    }

    let passwd = fs::read_to_string(passwd_path)
      .map_err(|e| CoreError::Custom(format!("failed to read {}: {e}", passwd_path.display())))?;
    for line in passwd.lines() {
      let line = line.trim();
      if line.is_empty() || line.starts_with('#') {
        continue;
      }
      if let Some(mut user) = UserRecord::from_passwd_line(line) {
        let idx = store.users.len();
        store.name_index.insert(user.username.clone(), idx);
        store.uid_index.insert(user.uid, idx);
        if let Some(perms) = user_perms.remove(&user.uid) {
          user.permissions = perms;
        }
        store.users.push(user);
      }
    }

    if let Ok(shadow) = fs::read_to_string(shadow_path) {
      for line in shadow.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
          continue;
        }
        if let Some(entry) = ShadowEntry::from_shadow_line(line) {
          store.shadows.push(entry);
        }
      }
    }

    if let Ok(group) = fs::read_to_string(group_path) {
      for line in group.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
          continue;
        }
        if let Some(mut entry) = GroupEntry::from_group_line(line) {
          if let Some(perms) = group_perms.remove(&entry.gid) {
            entry.permissions = perms;
          }
          store.groups.push(entry);
        }
      }
    }

    Ok(store)
  }

  pub fn lookup_by_name(&self, name: &str) -> Option<&UserRecord> {
    self.name_index.get(name).map(|&idx| &self.users[idx])
  }

  pub fn lookup_by_uid(&self, uid: u32) -> Option<&UserRecord> {
    self.uid_index.get(&uid).map(|&idx| &self.users[idx])
  }

  pub fn lookup_by_uid_mut(&mut self, uid: u32) -> Option<&mut UserRecord> {
    self.uid_index.get(&uid).map(|&idx| &mut self.users[idx])
  }

  pub fn group(&self, gid: u32) -> Option<&GroupEntry> {
    self.groups.iter().find(|g| g.gid == gid)
  }

  pub fn shadow_for(&self, username: &str) -> Option<&ShadowEntry> {
    self.shadows.iter().find(|s| s.username == username)
  }

  pub fn groups_for(&self, user: &UserRecord) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    if let Some(primary) = self.groups.iter().find(|g| g.gid == user.gid) {
      result.push(primary.name.clone());
    }
    for group in &self.groups {
      if group.members.iter().any(|m| m == &user.username) && !result.contains(&group.name) {
        result.push(group.name.clone());
      }
    }
    result
  }

  pub fn user_in_group(&self, user: &UserRecord, group_name: &str) -> bool {
    self.groups_for(user).iter().any(|g| g == group_name)
  }

  pub fn users(&self) -> &[UserRecord] {
    &self.users
  }

  pub fn write_perms_with_overlay(
    &self,
    perms_path: &Path,
    user_grants: &HashMap<u32, HashSet<u16>>,
    user_revokes: &HashMap<u32, HashSet<u16>>,
    group_grants: &HashMap<u32, HashSet<u16>>,
    group_revokes: &HashMap<u32, HashSet<u16>>,
  ) -> Result<(), CoreError> {
    let mut buf = Vec::new();

    for user in &self.users {
      let mut effective: BTreeSet<u16> = user.permissions.iter().copied().collect();

      if let Some(revokes) = user_revokes.get(&user.uid) {
        for perm in revokes {
          effective.remove(perm);
        }
      }
      if let Some(grants) = user_grants.get(&user.uid) {
        effective.extend(grants.iter().copied());
      }

      if effective.is_empty() {
        continue;
      }

      buf.push(0u8);
      buf.extend_from_slice(&user.uid.to_le_bytes());
      let count = u16::try_from(effective.len()).map_err(|_| {
        CoreError::Custom(format!(
          "too many permissions for user {} ({})",
          user.username, user.uid
        ))
      })?;
      buf.extend_from_slice(&count.to_le_bytes());
      for perm in effective {
        buf.extend_from_slice(&perm.to_le_bytes());
      }
    }

    for group in &self.groups {
      let mut effective: BTreeSet<u16> = group.permissions.iter().copied().collect();

      if let Some(revokes) = group_revokes.get(&group.gid) {
        for perm in revokes {
          effective.remove(perm);
        }
      }
      if let Some(grants) = group_grants.get(&group.gid) {
        effective.extend(grants.iter().copied());
      }

      if effective.is_empty() {
        continue;
      }

      buf.push(1u8);
      buf.extend_from_slice(&group.gid.to_le_bytes());
      let count = u16::try_from(effective.len()).map_err(|_| {
        CoreError::Custom(format!(
          "too many permissions for group {} ({})",
          group.name, group.gid
        ))
      })?;
      buf.extend_from_slice(&count.to_le_bytes());
      for perm in effective {
        buf.extend_from_slice(&perm.to_le_bytes());
      }
    }

    fs::write(perms_path, buf)
      .map_err(|e| CoreError::Custom(format!("failed to write {}: {e}", perms_path.display())))
  }
}

#[derive(Debug, Clone)]
pub enum PamError {
  UserNotFound,
  AccountLocked,
  AccountExpired,
  PasswordExpired,
  AuthenticationFailed,
  MaxRetriesExceeded,
  SessionError(String),
  InternalError(String),
}

impl std::fmt::Display for PamError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::UserNotFound => write!(f, "user not found"),
      Self::AccountLocked => write!(f, "account is locked"),
      Self::AccountExpired => write!(f, "account has expired"),
      Self::PasswordExpired => write!(f, "password has expired"),
      Self::AuthenticationFailed => write!(f, "authentication failed"),
      Self::MaxRetriesExceeded => write!(f, "maximum login retries exceeded"),
      Self::SessionError(e) => write!(f, "session error: {e}"),
      Self::InternalError(e) => write!(f, "internal error: {e}"),
    }
  }
}

pub fn verify_password(password: &str, hash: &str) -> bool {
  if hash.is_empty() || hash == "!" || hash == "*" || hash == "!!" {
    return false;
  }

  if hash.starts_with("$6$") {
    sha_crypt::sha512_check(password, hash).is_ok()
  } else if hash.starts_with("$5$") {
    sha_crypt::sha256_check(password, hash).is_ok()
  } else {
    false
  }
}

static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct PamSession {
  pub id: u64,
  pub username: String,
  pub tty: String,
  pub started_at: Instant,
}

impl PamSession {
  fn new(username: String, tty: String) -> Self {
    Self {
      id: SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
      username,
      tty,
      started_at: Instant::now(),
    }
  }
}

#[derive(Debug, Clone)]
struct LockoutState {
  failed_attempts: u32,
  last_failure: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct PamConfig {
  pub max_retries: u32,
  pub lockout_duration: Duration,
}

impl Default for PamConfig {
  fn default() -> Self {
    Self {
      max_retries: 5,
      lockout_duration: Duration::from_secs(60),
    }
  }
}

pub struct PamHandle {
  store: UserStoreShared,
  config: PamConfig,
  sessions: Arc<RwLock<HashMap<u64, PamSession>>>,
  lockouts: Arc<RwLock<HashMap<String, LockoutState>>>,
}

impl PamHandle {
  pub fn new(store: UserStoreShared) -> Self {
    Self::with_config(store, PamConfig::default())
  }

  pub fn with_config(store: UserStoreShared, config: PamConfig) -> Self {
    Self {
      store,
      config,
      sessions: Arc::new(RwLock::new(HashMap::new())),
      lockouts: Arc::new(RwLock::new(HashMap::new())),
    }
  }

  pub fn store(&self) -> &UserStore {
    &self.store
  }

  pub fn sessions(&self) -> &Arc<RwLock<HashMap<u64, PamSession>>> {
    &self.sessions
  }

  pub fn pam_authenticate(&self, username: &str, password: &str) -> Result<&UserRecord, PamError> {
    self.check_lockout(username)?;

    let user = self
      .store
      .lookup_by_name(username)
      .ok_or(PamError::UserNotFound)?;

    let shadow = self
      .store
      .shadow_for(username)
      .ok_or(PamError::UserNotFound)?;

    if shadow.is_locked() {
      return Err(PamError::AccountLocked);
    }

    if !verify_password(password, &shadow.password_hash) {
      self.record_failure(username);
      return Err(PamError::AuthenticationFailed);
    }

    self.clear_lockout(username);

    Ok(user)
  }

  pub fn pam_authenticate_auto(&self, username: &str) -> Result<&UserRecord, PamError> {
    let user = self
      .store
      .lookup_by_name(username)
      .ok_or(PamError::UserNotFound)?;

    let shadow = self
      .store
      .shadow_for(username)
      .ok_or(PamError::UserNotFound)?;

    if shadow.is_locked() {
      return Err(PamError::AccountLocked);
    }

    Ok(user)
  }

  pub fn pam_acct_mgmt(&self, username: &str) -> Result<(), PamError> {
    let shadow = self
      .store
      .shadow_for(username)
      .ok_or(PamError::UserNotFound)?;

    if shadow.is_locked() {
      return Err(PamError::AccountLocked);
    }
    if shadow.is_expired() {
      return Err(PamError::AccountExpired);
    }
    if shadow.password_expired() {
      return Err(PamError::PasswordExpired);
    }

    Ok(())
  }

  pub fn pam_open_session(&self, username: &str, tty: &str) -> Result<PamSession, PamError> {
    let _ = self
      .store
      .lookup_by_name(username)
      .ok_or(PamError::UserNotFound)?;

    let session = PamSession::new(username.to_string(), tty.to_string());
    let id = session.id;

    let mut sessions = self
      .sessions
      .write()
      .map_err(|e| PamError::SessionError(e.to_string()))?;
    sessions.insert(id, session.clone());

    Ok(session)
  }

  pub fn pam_close_session(&self, session_id: u64) -> Result<(), PamError> {
    let mut sessions = self
      .sessions
      .write()
      .map_err(|e| PamError::SessionError(e.to_string()))?;
    sessions
      .remove(&session_id)
      .ok_or(PamError::SessionError("session not found".into()))?;
    Ok(())
  }

  pub fn sessions_for(&self, username: &str) -> Vec<PamSession> {
    let Ok(sessions) = self.sessions.read() else {
      return Vec::new();
    };
    sessions
      .values()
      .filter(|s| s.username == username)
      .cloned()
      .collect()
  }

  pub fn has_active_session(&self, username: &str) -> bool {
    let Ok(sessions) = self.sessions.read() else {
      return false;
    };
    sessions.values().any(|s| s.username == username)
  }

  fn check_lockout(&self, username: &str) -> Result<(), PamError> {
    let Ok(lockouts) = self.lockouts.read() else {
      return Ok(());
    };
    if let Some(state) = lockouts.get(username) {
      if state.failed_attempts >= self.config.max_retries {
        if let Some(last) = state.last_failure {
          if last.elapsed() < self.config.lockout_duration {
            return Err(PamError::MaxRetriesExceeded);
          }
        }
      }
    }
    Ok(())
  }

  fn record_failure(&self, username: &str) {
    let Ok(mut lockouts) = self.lockouts.write() else {
      return;
    };
    let state = lockouts
      .entry(username.to_string())
      .or_insert(LockoutState {
        failed_attempts: 0,
        last_failure: None,
      });
    state.failed_attempts += 1;
    state.last_failure = Some(Instant::now());
  }

  fn clear_lockout(&self, username: &str) {
    let Ok(mut lockouts) = self.lockouts.write() else {
      return;
    };
    lockouts.remove(username);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;

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

  fn make_test_store(dir: &Path) -> UserStoreShared {
    let hash = generate_test_hash("test123");

    write_file(
      dir,
      "etc/passwd",
      &format!(
        "root:x:0:0:root:/root:/bin/sh\nmakano:x:1000:1000:Makano:/home/makano:/bin/sh\nlocked:x:1001:1001:Locked:/home/locked:/bin/sh\n"
      ),
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

  fn generate_test_hash(password: &str) -> String {
    let params = sha_crypt::Sha512Params::new(5000).expect("valid rounds");
    sha_crypt::sha512_simple(password, &params).expect("hash generation")
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
    assert!(groups.contains(&"makano".to_string()));
    assert!(groups.contains(&"wheel".to_string()));
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
    assert_eq!(user.username, "makano");

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
    assert_eq!(user.username, "makano");

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
      username: "test".to_string(),
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
}
