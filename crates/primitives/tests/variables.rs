use rind_primitives::variables::VariableHeap;

fn tmp_path(tag: &str) -> std::path::PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock is before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!("rind-vars-{tag}-{}-{now}.toml", std::process::id()))
}

#[test]
fn variable_heap_prefers_explicit_value_then_env_then_default() {
  let path = tmp_path("precedence");
  let mut heap = VariableHeap::new(&path);
  heap.register(
    "answer",
    Some(toml::Value::Integer(10)),
    Some("RIND_TEST_ANSWER".into()),
  );

  unsafe { std::env::set_var("RIND_TEST_ANSWER", "22") };
  assert_eq!(
    heap.get("answer"),
    Some(toml::Value::String("22".to_string()))
  );

  heap.set("answer", toml::Value::Integer(33));
  assert_eq!(heap.get("answer"), Some(toml::Value::Integer(33)));
  unsafe { std::env::remove_var("RIND_TEST_ANSWER") };
  let _ = std::fs::remove_file(path);
}

#[test]
fn variable_heap_save_load_roundtrip() {
  let path = tmp_path("roundtrip");
  let mut heap = VariableHeap::new(&path);
  heap.set("enabled", toml::Value::Boolean(true));
  heap.set("port", toml::Value::Integer(8080));
  heap.save().expect("save should succeed");

  let mut loaded = VariableHeap::new(&path);
  loaded.load().expect("load should succeed");
  assert_eq!(loaded.get("enabled"), Some(toml::Value::Boolean(true)));
  assert_eq!(loaded.get("port"), Some(toml::Value::Integer(8080)));

  let _ = std::fs::remove_file(path);
}
