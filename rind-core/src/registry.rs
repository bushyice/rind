use std::{
  any::{Any, TypeId},
  collections::HashMap,
};

use crate::{
  error::CoreError,
  metadata::{Metadata, Model, NamedItem},
};

#[derive(Default)]
pub struct MetadataRegistry {
  metadata: HashMap<String, Metadata>,
  indexes: HashMap<TypeId, HashMap<String, usize>>,
}

impl MetadataRegistry {
  pub fn meta() {}

  pub fn insert_metadata(&mut self, metadata: Metadata) {
    self.metadata.insert(metadata.name.clone(), metadata);
  }

  pub fn load_group_from_toml(
    &mut self,
    metadata: &str,
    group: &str,
    source: &str,
  ) -> anyhow::Result<()> {
    let m = self
      .metadata
      .get_mut(metadata)
      .ok_or(CoreError::MetadataNotFound(metadata.to_string()))?;
    m.from_toml(source, group)?;
    self.indexes.clear();
    Ok(())
  }

  pub fn group_items<T: Model + 'static>(&self, metadata: &str, group: &str) -> Option<&Vec<T::M>> {
    self.metadata.get(metadata)?.get_in_group::<T>(group)
  }

  fn ensure_index_for_type<T>(&mut self, metadata: &str) -> anyhow::Result<()>
  where
    T: Model + 'static,
  {
    let type_id = TypeId::of::<T>();
    if self.indexes.contains_key(&type_id) {
      return Ok(());
    }

    let m = self
      .metadata
      .get_mut(metadata)
      .ok_or(CoreError::MetadataNotFound(metadata.to_string()))?;

    let mut map = HashMap::new();
    for group in m.groups() {
      if let Some(items) = m.get_in_group::<T>(group) {
        for (idx, item) in items.iter().enumerate() {
          map.insert(format!("{group}@{}", item.name()), idx);
        }
      }
    }

    self.indexes.insert(type_id, map);

    Ok(())
  }

  pub fn find<T>(&mut self, metadata: &str, full_name: &str) -> Option<&T::M>
  where
    T: Model + 'static,
  {
    let (group, _) = full_name.split_once('@')?;
    self.ensure_index_for_type::<T>(metadata).ok()?;

    let idx = *self.indexes.get(&TypeId::of::<T>())?.get(full_name)?;
    self.group_items::<T>(metadata, group)?.get(idx)
  }

  pub fn lookup<T>(&self, metadata: &str, full_name: &str) -> Option<&T::M>
  where
    T: Model + 'static,
  {
    let (group, item_name) = full_name.split_once('@')?;
    self
      .group_items::<T>(metadata, group)?
      .iter()
      .find(|item| item.name() == item_name)
  }

  pub fn metadata(&self, metadata: &str) -> Option<&Metadata> {
    self.metadata.get(metadata)
  }

  pub fn metadata_mut(&mut self, metadata: &str) -> Option<&mut Metadata> {
    self.metadata.get_mut(metadata)
  }
}

#[derive(Default)]
pub struct InstanceRegistry {
  pub metadata: MetadataRegistry,
  pub instances: HashMap<String, Vec<Box<dyn Any>>>,
}

impl InstanceRegistry {
  pub fn instantiate<T>(
    &mut self,
    metadata: &str,
    name: &str,
    instantiate: impl Fn(&T::M) -> anyhow::Result<T>,
  ) -> anyhow::Result<&Box<T>>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    let metadata = self
      .metadata
      .find::<T>(metadata, name)
      .ok_or(CoreError::MetadataNotFound(metadata.to_string()))?;

    let entry = self.instances.entry(full_name).or_default();

    let instance = instantiate(metadata)?;

    entry.push(Box::new(instance));

    let last = entry.last().unwrap();

    Ok(last.downcast_ref::<Box<T>>().unwrap())
  }

  pub fn instances<T>(&self, metadata: &str, name: &str) -> anyhow::Result<Vec<&Box<T>>>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    Ok(
      self
        .instances
        .get(&full_name)
        .ok_or(CoreError::MissingInstances(full_name))?
        .iter()
        .map(|x| x.downcast_ref::<Box<T>>().unwrap())
        .collect(),
    )
  }

  pub fn instances_mut<T>(&mut self, metadata: &str, name: &str) -> anyhow::Result<Vec<&mut Box<T>>>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    Ok(
      self
        .instances
        .get_mut(&full_name)
        .ok_or(CoreError::MissingInstances(full_name.to_string()))?
        .iter_mut()
        .map(|x| x.downcast_mut::<Box<T>>().unwrap())
        .collect(),
    )
  }

  pub fn as_one<T>(&self, metadata: &str, name: &str) -> anyhow::Result<&Box<T>>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    let instances = self
      .instances
      .get(&full_name)
      .ok_or(CoreError::MissingInstances(full_name))?;

    Ok(instances.last().unwrap().downcast_ref::<Box<T>>().unwrap())
  }

  pub fn as_one_mut<T>(&mut self, metadata: &str, name: &str) -> anyhow::Result<&mut Box<T>>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    let instances = self
      .instances
      .get_mut(&full_name)
      .ok_or(CoreError::MissingInstances(full_name))?;

    Ok(
      instances
        .last_mut()
        .unwrap()
        .downcast_mut::<Box<T>>()
        .unwrap(),
    )
  }
}

#[cfg(test)]
mod tests {
  use super::super::metadata::*;
  use super::*;
  use rind_macros::model;

  #[model(meta_name = name, meta_fields(name, run))]
  struct Service {
    name: String,
    run: Option<String>,
  }

  #[model(meta_name = target, meta_fields(target))]
  struct Mount {
    target: String,
  }

  #[test]
  fn lookup_by_group_and_item_name() {
    let metadata = Metadata::new("units")
      .of::<Service>("service")
      .of::<Mount>("mount");

    let mut registry = MetadataRegistry::default();

    registry.insert_metadata(metadata);

    let src = r#"
[[service]]
name = "web"
run = "/bin/webd"

[[service]]
name = "api"
"#;

    registry
      .load_group_from_toml("units", "demo", src)
      .expect("group should parse");

    let web = registry
      .find::<Service>("units", "demo@web")
      .expect("service should be indexed by unit@item");
    assert_eq!(web.name, "web");
    assert_eq!(web.run.as_deref(), Some("/bin/webd"));

    assert!(registry.find::<Service>("units", "demo@missing").is_none());
    assert!(registry.find::<Service>("units", "missing@web").is_none());
  }
}
