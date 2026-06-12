//! TODO: Better state saving

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::types::Void;

pub type StateSnapshot = HashMap<String, Vec<StateEntry>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEntry {
  pub data: Vec<u8>,
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
  pub const KEY: &str = "runtime:state_persistence";

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

    let mut file =
      fs::File::open(path).map_err(|e| CoreError::PersistenceError(format!("open failed: {e}")))?;
    let mut content = Vec::new();
    file
      .read_to_end(&mut content)
      .map_err(|e| CoreError::PersistenceError(format!("read failed: {e}")))?;

    let snapshot = decode_snapshot(&content)?;

    Ok(snapshot)
  }

  pub fn save(&self, snapshot: StateSnapshot) {
    let _ = self.tx.send(PersistCommand::Save(snapshot));
  }

  pub fn save_sync(&self, snapshot: &StateSnapshot) -> Result<Void, CoreError> {
    write_snapshot(&self.path, snapshot)
  }

  pub fn shutdown(&self) {
    let _ = self.tx.send(PersistCommand::Shutdown);
  }
}

fn write_snapshot(path: &Path, snapshot: &StateSnapshot) -> Result<Void, CoreError> {
  let encoded = encode_snapshot(snapshot)?;

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
  sync_parent_dir(path)?;

  Ok(Void)
}

const MAGIC: [u8; 4] = *b"RIND";
const VERSION: u16 = 1;

fn encode_snapshot(snapshot: &StateSnapshot) -> Result<Vec<u8>, CoreError> {
  let cfg = bincode_next::config::standard();
  let payload = bincode_next::serde::encode_to_vec(snapshot, cfg)
    .map_err(|e| CoreError::PersistenceError(format!("encode failed: {e}")))?;

  let checksum = crc32fast::hash(&payload);
  let mut out = Vec::with_capacity(4 + 2 + 4 + payload.len());
  out.extend_from_slice(&MAGIC);
  out.extend_from_slice(&VERSION.to_le_bytes());
  out.extend_from_slice(&checksum.to_le_bytes());
  out.extend_from_slice(&payload);
  Ok(out)
}

fn decode_snapshot(content: &[u8]) -> Result<StateSnapshot, CoreError> {
  if content.len() < 10 {
    return Err(CoreError::PersistenceError(
      "decode failed: snapshot too small".to_string(),
    ));
  }

  if content[0..4] != MAGIC {
    return Err(CoreError::PersistenceError(
      "decode failed: invalid snapshot magic".to_string(),
    ));
  }

  let version = u16::from_le_bytes([content[4], content[5]]);
  if version != VERSION {
    return Err(CoreError::PersistenceError(format!(
      "decode failed: unsupported snapshot version {version}"
    )));
  }

  let expected = u32::from_le_bytes([content[6], content[7], content[8], content[9]]);
  let payload = &content[10..];
  let actual = crc32fast::hash(payload);
  if expected != actual {
    return Err(CoreError::PersistenceError(
      "decode failed: snapshot checksum mismatch".to_string(),
    ));
  }

  let cfg = bincode_next::config::standard();
  let (snapshot, _): (StateSnapshot, usize) = bincode_next::serde::decode_from_slice(payload, cfg)
    .map_err(|e| CoreError::PersistenceError(format!("decode failed: {e}")))?;
  Ok(snapshot)
}

fn sync_parent_dir(path: &Path) -> Result<Void, CoreError> {
  let Some(parent) = path.parent() else {
    return Ok(Void);
  };
  let dir = fs::File::open(parent)
    .map_err(|e| CoreError::PersistenceError(format!("open parent dir failed: {e}")))?;
  dir
    .sync_all()
    .map_err(|e| CoreError::PersistenceError(format!("sync parent dir failed: {e}")))
}
