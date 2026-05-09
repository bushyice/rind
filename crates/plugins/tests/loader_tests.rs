use std::fs;
use std::path::PathBuf;

use rind_core::prelude::{LogConfig, start_logger};
use rind_plugins::{collect_plugins, plugins_path};

fn temp_dir(tag: &str) -> PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!(
    "rind-plugin-tests-{tag}-{}-{now}",
    std::process::id()
  ))
}

#[test]
fn collect_plugins_skips_invalid_shared_objects_without_panicking() {
  let dir = temp_dir("invalid-so");
  fs::create_dir_all(&dir).expect("temp plugin dir should exist");
  fs::write(dir.join("broken.so"), b"not a shared object").expect("fixture should be created");

  let log = start_logger(LogConfig {
    dir: temp_dir("logs"),
    ..LogConfig::default()
  });

  let plugins: Vec<_> = collect_plugins(&dir, &log)
    .expect("plugin collection should still succeed")
    .collect();
  assert!(plugins.is_empty());
}

#[test]
fn plugins_path_uses_env_override() {
  unsafe { std::env::set_var("RIND_VARIABLES_PATH", "/tmp/rind-plugin-path-override") };
  assert_eq!(
    plugins_path(),
    PathBuf::from("/tmp/rind-plugin-path-override")
  );
  unsafe { std::env::remove_var("RIND_VARIABLES_PATH") };
}
