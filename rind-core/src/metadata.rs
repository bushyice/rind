use std::{
  any::{Any, TypeId},
  collections::HashMap,
};

use toml::Value;

pub trait NamedItem {
  fn name(&self) -> &str;
}

pub trait Model {
  type M: serde::de::DeserializeOwned + NamedItem;
}

type ParserFn = Box<dyn Fn(Value) -> anyhow::Result<Box<dyn Any>> + Send + Sync>;

pub struct Metadata {
  pub name: String,
  name_to_type: HashMap<String, TypeId>,
  parsers: HashMap<TypeId, ParserFn>,
  values: HashMap<String, HashMap<TypeId, Box<dyn Any>>>,
}

impl Metadata {
  pub fn new(name: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      name_to_type: HashMap::new(),
      parsers: HashMap::new(),
      values: HashMap::new(),
    }
  }

  pub fn of<T>(mut self, name: &str) -> Self
  where
    T: Model + 'static,
  {
    let type_id = TypeId::of::<T::M>();

    self.name_to_type.insert(name.to_string(), type_id);
    self.parsers.insert(
      type_id,
      Box::new(|value| {
        let parsed: Vec<T::M> = value.try_into()?;
        Ok(Box::new(parsed))
      }),
    );

    self
  }

  pub fn from_toml(&mut self, toml: &str, group: &str) -> anyhow::Result<()> {
    let value: Value = toml::from_str(toml)?;
    self.collect_value(value, group)
  }

  pub fn collect_value(&mut self, value: Value, group: &str) -> anyhow::Result<()> {
    let table = value
      .as_table()
      .ok_or_else(|| anyhow::anyhow!("root must be table"))?;

    for (key, val) in table {
      if matches!(val, Value::Array(_)) && self.name_to_type.contains_key(key) {
        self.insert_value(key, val.clone(), group)?;
      }
    }

    Ok(())
  }

  pub fn insert_value(&mut self, name: &str, value: Value, group: &str) -> anyhow::Result<()> {
    let type_id = *self
      .name_to_type
      .get(name)
      .ok_or_else(|| anyhow::anyhow!("unknown metadata key `{name}`"))?;
    let parser = self
      .parsers
      .get(&type_id)
      .ok_or_else(|| anyhow::anyhow!("missing parser for `{name}`"))?;

    let parsed = parser(value)?;
    self
      .values
      .entry(group.to_string())
      .or_default()
      .insert(type_id, parsed);

    Ok(())
  }

  pub fn get_in_group<T: Model + 'static>(&self, group: &str) -> Option<&Vec<T::M>> {
    self
      .values
      .get(group)?
      .get(&TypeId::of::<T::M>())?
      .downcast_ref::<Vec<T::M>>()
  }

  pub fn groups(&self) -> impl Iterator<Item = &str> {
    self.values.keys().map(|k| k.as_str())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use rind_macros::model;
  use toml::Value;

  #[model(meta_name = name, meta_fields(name, run))]
  struct Service {
    name: String,
  }

  #[test]
  fn parse_grouped_metadata_from_toml() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let src = r#"
[[service]]
name = "web"
run = "/bin/webd"

[[service]]
name = "api"
"#;

    metadata
      .from_toml(src, "demo")
      .expect("toml should parse into group");

    let services = metadata
      .get_in_group::<Service>("demo")
      .expect("service vec should exist in group");
    assert_eq!(services.len(), 2);
    assert_eq!(services[0].name, "web");
    assert_eq!(services[1].name, "api");
  }

  #[test]
  fn insert_value_type_mismatch_errors() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let err = metadata
      .insert_value("service", Value::String("not-an-array".to_string()), "demo")
      .expect_err("non-array value should fail Vec<Service> parser");

    assert!(!err.to_string().is_empty());
  }

  #[test]
  fn unknown_key_is_ignored_by_collect() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let src = r#"
[[mount]]
name = "data"
"#;

    metadata
      .from_toml(src, "demo")
      .expect("unknown top-level arrays are ignored");
    assert!(metadata.get_in_group::<Service>("demo").is_none());
  }
}
