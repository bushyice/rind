use rind_core::prelude::rslvns;
use rind_core::prelude::{StateEntry, StatePersistence, StateSnapshot};
use std::thread;

fn temp_path() -> std::path::PathBuf {
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
    rslvns!("test", "active").into(),
    vec![StateEntry {
      data: b"hello".to_vec(),
    }],
  );

  persistence.save_sync(&snapshot).expect("save should work");

  let loaded = persistence.load().expect("load should work");
  assert_eq!(loaded.len(), 1);
  assert!(loaded.contains_key(&rslvns!("test", "active")));

  let _ = std::fs::remove_file(path);
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
    rslvns!("async", "test").into(),
    vec![StateEntry {
      data: b"hello".to_vec(),
    }],
  );

  persistence.save(snapshot);

  thread::sleep(std::time::Duration::from_millis(100));

  let loaded = persistence.load().expect("should load async save");
  assert!(loaded.contains_key(&rslvns!("async", "test")));

  let _ = std::fs::remove_file(path);
  persistence.shutdown();
}
