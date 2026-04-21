use rind_core::prelude::*;

#[model(meta_name = name, meta_fields(name, env, default), derive_metadata(Debug, Clone))]
pub struct Variable {
  pub name: String,
  pub default: Option<toml::Value>,
  pub env: Option<String>,
}

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use rind_core::error::CoreError;

#[derive(Clone)]
pub struct VariableHeap {
  values: HashMap<String, toml::Value>,
  defaults: HashMap<String, toml::Value>,
  env_mappings: HashMap<String, String>,
  path: PathBuf,
}

impl VariableHeap {
  pub const KEY: &str = "runtime@variable_heap";

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
        self.values.insert(key, value);
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
      table.insert(key.clone(), value.clone());
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

  pub fn register(&mut self, id: &str, default: Option<toml::Value>, env: Option<String>) {
    self.defaults.insert(
      id.to_string(),
      default.unwrap_or(toml::Value::Boolean(false)),
    );
    if let Some(env_name) = env {
      self.env_mappings.insert(id.to_string(), env_name);
    }
  }

  pub fn set(&mut self, id: &str, value: toml::Value) {
    self.values.insert(id.to_string(), value);
  }

  pub fn get(&self, id: &str) -> Option<toml::Value> {
    if let Some(val) = self.values.get(id) {
      return Some(val.clone());
    }

    if let Some(env_name) = self.env_mappings.get(id) {
      if let Ok(val) = std::env::var(env_name) {
        return Some(toml::Value::String(val));
      }
    }

    self.defaults.get(id).cloned()
  }

  pub fn contains(&self, id: &str) -> bool {
    self.defaults.contains_key(id)
  }
}

pub fn variables_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_VARIABLES_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/lib/rind/variables.toml")
  }
}
