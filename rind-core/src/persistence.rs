//! TODO: Better state saving

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

pub type StateSnapshot = HashMap<String, Vec<StateEntry>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEntry {
  pub name: String,
  pub payload: serde_json::Value,
}

enum PersistCommand {
  Save(StateSnapshot),
  Shutdown,
}

#[derive(Clone)]
pub struct StatePersistence {
  path: PathBuf,
  tx: Sender<PersistCommand>,
}

impl StatePersistence {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    let path = path.into();
    let (tx, rx) = mpsc::channel();

    let writer_path = path.clone();
    thread::spawn(move || {
      for cmd in rx {
        match cmd {
          PersistCommand::Save(snapshot) => {
            if let Err(e) = write_snapshot(&writer_path, &snapshot) {
              eprintln!("[persistence] save failed: {e}");
            }
          }
          PersistCommand::Shutdown => break,
        }
      }
    });

    Self { path, tx }
  }

  pub fn load(&self) -> Result<StateSnapshot, CoreError> {
    let path = &self.path;
    if !path.exists() {
      return Ok(StateSnapshot::default());
    }

    let content = fs::read_to_string(path)
      .map_err(|e| CoreError::PersistenceError(format!("read failed: {e}")))?;

    let snapshot: StateSnapshot = serde_json::from_str(&content)
      .map_err(|e| CoreError::PersistenceError(format!("decode failed: {e}")))?;

    Ok(snapshot)
  }

  pub fn save(&self, snapshot: StateSnapshot) {
    let _ = self.tx.send(PersistCommand::Save(snapshot));
  }

  pub fn save_sync(&self, snapshot: &StateSnapshot) -> Result<(), CoreError> {
    write_snapshot(&self.path, snapshot)
  }

  pub fn shutdown(&self) {
    let _ = self.tx.send(PersistCommand::Shutdown);
  }
}

fn write_snapshot(path: &Path, snapshot: &StateSnapshot) -> Result<(), CoreError> {
  let encoded = serde_json::to_vec(snapshot)
    .map_err(|e| CoreError::PersistenceError(format!("encode failed: {e}")))?;

  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .map_err(|e| CoreError::PersistenceError(format!("create dir failed: {e}")))?;
  }

  let tmp = path.with_extension("tmp");
  let mut file = fs::File::create(&tmp)
    .map_err(|e| CoreError::PersistenceError(format!("create tmp failed: {e}")))?;
  file
    .write_all(&encoded)
    .map_err(|e| CoreError::PersistenceError(format!("write failed: {e}")))?;
  file
    .sync_all()
    .map_err(|e| CoreError::PersistenceError(format!("sync failed: {e}")))?;

  fs::rename(&tmp, path).map_err(|e| CoreError::PersistenceError(format!("rename failed: {e}")))?;

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;

  fn temp_path() -> PathBuf {
    std::env::temp_dir().join(format!(
      "rind-persist-test-{}-{}.state",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
    ))
  }

  #[test]
  fn roundtrip_save_load() {
    let path = temp_path();
    let persistence = StatePersistence::new(path.clone());

    let mut snapshot = StateSnapshot::new();
    snapshot.insert(
      "test@active".into(),
      vec![StateEntry {
        name: "active".into(),
        payload: serde_json::json!({"id": "unit_a"}),
      }],
    );

    persistence.save_sync(&snapshot).expect("save should work");

    let loaded = persistence.load().expect("load should work");
    assert_eq!(loaded.len(), 1);
    assert!(loaded.contains_key("test@active"));

    let _ = fs::remove_file(path);
    persistence.shutdown();
  }

  #[test]
  fn load_missing_file_returns_empty() {
    let path = temp_path();
    let persistence = StatePersistence::new(path);

    let loaded = persistence.load().expect("should not error");
    assert!(loaded.is_empty());
    persistence.shutdown();
  }

  #[test]
  fn async_save() {
    let path = temp_path();
    let persistence = StatePersistence::new(path.clone());

    let mut snapshot = StateSnapshot::new();
    snapshot.insert(
      "async@test".into(),
      vec![StateEntry {
        name: "test".into(),
        payload: serde_json::json!("hello"),
      }],
    );

    persistence.save(snapshot);

    thread::sleep(Duration::from_millis(100));

    let loaded = persistence.load().expect("should load async save");
    assert!(loaded.contains_key("async@test"));

    let _ = fs::remove_file(path);
    persistence.shutdown();
  }
}
