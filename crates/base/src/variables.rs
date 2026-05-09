use rind_core::prelude::*;

#[model(meta_name = name, meta_fields(name, env, default), derive_metadata(Debug, Clone))]
pub struct Variable {
  pub name: Ustr,
  pub default: Option<toml::Value>,
  pub env: Option<Ustr>,
}

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use rind_core::error::CoreError;

#[derive(Clone)]
pub struct VariableHeap {
  values: HashMap<Ustr, toml::Value>,
  defaults: HashMap<Ustr, toml::Value>,
  env_mappings: HashMap<Ustr, Ustr>,
  path: PathBuf,
}

impl VariableHeap {
  pub const KEY: &str = "runtime:variable_heap";

  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      values: HashMap::new(),
      defaults: HashMap::new(),
      env_mappings: HashMap::new(),
      path: path.into(),
    }
  }

  pub fn load(&mut self) -> Result<(), CoreError> {
    if !self.path.exists() {
      return Ok(());
    }

    let content = fs::read_to_string(&self.path).map_err(|e| {
      CoreError::PersistenceError(format!(
        "failed to read variables file {}: {e}",
        self.path.display()
      ))
    })?;

    let table: toml::Value = toml::from_str(&content).map_err(|e| {
      CoreError::PersistenceError(format!(
        "failed to parse variables file {}: {e}",
        self.path.display()
      ))
    })?;

    if let toml::Value::Table(map) = table {
      for (key, value) in map {
        self.values.insert(Ustr::from(key), value);
      }
    }

    Ok(())
  }

  pub fn save(&self) -> Result<(), CoreError> {
    if let Some(parent) = self.path.parent() {
      fs::create_dir_all(parent)
        .map_err(|e| CoreError::PersistenceError(format!("failed to create variables dir: {e}")))?;
    }

    let mut table = toml::map::Map::new();
    for (key, value) in &self.values {
      table.insert(key.to_string(), value.clone());
    }

    let content = toml::to_string_pretty(&toml::Value::Table(table))
      .map_err(|e| CoreError::PersistenceError(format!("failed to serialize variables: {e}")))?;

    let tmp = self.path.with_extension("tmp");
    fs::write(&tmp, content.as_bytes())
      .map_err(|e| CoreError::PersistenceError(format!("failed to write variables tmp: {e}")))?;
    fs::rename(&tmp, &self.path)
      .map_err(|e| CoreError::PersistenceError(format!("failed to rename variables: {e}")))?;

    Ok(())
  }

  pub fn register(&mut self, id: impl Into<Ustr>, default: Option<toml::Value>, env: Option<Ustr>) {
    let id = id.into();
    self
      .defaults
      .insert(id.clone(), default.unwrap_or(toml::Value::Boolean(false)));
    if let Some(env_name) = env {
      self.env_mappings.insert(id, env_name);
    }
  }

  pub fn set(&mut self, id: impl Into<Ustr>, value: toml::Value) {
    self.values.insert(id.into(), value);
  }

  pub fn get(&self, id: &str) -> Option<toml::Value> {
    let id_ustr = Ustr::from(id);
    if let Some(val) = self.values.get(&id_ustr) {
      return Some(val.clone());
    }

    if let Some(env_name) = self.env_mappings.get(&id_ustr) {
      if let Ok(val) = std::env::var(env_name.as_str()) {
        return Some(toml::Value::String(val));
      }
    }

    self.defaults.get(&id_ustr).cloned()
  }

  pub fn contains(&self, id: &str) -> bool {
    self.defaults.contains_key(&Ustr::from(id))
  }
}

pub fn variables_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_VARIABLES_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/lib/rind/variables.toml")
  }
}

#[cfg(test)]
mod tests {
  use super::VariableHeap;

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
}
