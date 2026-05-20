use rind_primitives::variables::VariableHeap;

fn tmp_path(tag: &str) -> std::path::PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock is before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!(
    "rind-var-contract-{tag}-{}-{now}.toml",
    std::process::id()
  ))
}

#[test]
fn variable_heap_contract_persistence_and_precedence() {
  let path = tmp_path("persistence");

  let mut heap = VariableHeap::new(&path);
  heap.register(
    "mode",
    Some(toml::Value::String("default".to_string())),
    Some("RIND_CONTRACT_MODE".into()),
  );

  unsafe { std::env::set_var("RIND_CONTRACT_MODE", "env") };
  assert_eq!(
    heap.get("mode"),
    Some(toml::Value::String("env".to_string()))
  );

  heap.set("mode", toml::Value::String("explicit".to_string()));
  heap.save().expect("save should succeed");

  let mut loaded = VariableHeap::new(&path);
  loaded.register(
    "mode",
    Some(toml::Value::String("default".to_string())),
    Some("RIND_CONTRACT_MODE".into()),
  );
  loaded.load().expect("load should succeed");

  assert_eq!(
    loaded.get("mode"),
    Some(toml::Value::String("explicit".to_string()))
  );

  unsafe { std::env::remove_var("RIND_CONTRACT_MODE") };
  let _ = std::fs::remove_file(path);
}
