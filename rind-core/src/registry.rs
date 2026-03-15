use std::{
  any::{Any, TypeId},
  collections::HashMap,
  sync::Arc,
};

use crate::{
  error::CoreError,
  metadata::{Metadata, Model, NamedItem},
};

#[derive(Default)]
pub struct MetadataRegistry {
  metadata: HashMap<String, Arc<Metadata>>,
  pub indexes: HashMap<TypeId, HashMap<String, usize>>,
}

impl MetadataRegistry {
  pub fn insert_metadata(&mut self, metadata: Metadata) {
    self
      .metadata
      .insert(metadata.name.clone(), Arc::new(metadata));
  }

  pub fn load_group_from_toml(
    &mut self,
    metadata: &mut Metadata,
    group: &str,
    source: &str,
  ) -> anyhow::Result<()> {
    metadata.from_toml(source, group)?;
    self.indexes.clear();
    Ok(())
  }

  pub fn group_items<T: Model + 'static>(
    &self,
    metadata: &str,
    group: &str,
  ) -> Option<Vec<Arc<T::M>>> {
    self
      .metadata
      .get(metadata)?
      .get_in_group::<T>(group)
      .map(|x| x.iter().map(|x| x.clone()).collect())
  }

  pub fn ensure_index_for_type<T>(&mut self, metadata: &str) -> anyhow::Result<()>
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

  pub fn find<T>(&self, metadata: &str, full_name: &str) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let (group, _) = full_name.split_once('@')?;

    let idx = *self.indexes.get(&TypeId::of::<T>())?.get(full_name)?;
    self
      .group_items::<T>(metadata, group)?
      .get(idx)
      .map(|x| x.clone())
  }

  pub fn lookup<T>(&self, metadata: &str, full_name: &str) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let (group, item_name) = full_name.split_once('@')?;
    self
      .group_items::<T>(metadata, group)?
      .iter()
      .find(|item| item.name() == item_name)
      .map(|x| x.clone())
  }

  pub fn lookup_in_any_group<T>(&self, metadata: &str, item_name: &str) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let m = self.metadata.get(metadata)?;
    for group in m.groups() {
      let full = format!("{group}@{item_name}");
      if let Some(found) = self.lookup::<T>(metadata, full.as_str()) {
        return Some(found);
      }
    }
    None
  }

  pub fn metadata(&self, metadata: &str) -> Option<Arc<Metadata>> {
    self.metadata.get(metadata).map(|x| x.clone())
  }
}

pub type InstanceMap = HashMap<String, Vec<Box<dyn Any>>>;

pub struct InstanceRegistry<'a> {
  pub metadata: &'a MetadataRegistry,
  pub instances: &'a mut InstanceMap,
}

impl<'a> InstanceRegistry<'a> {
  pub fn new(metadata: &'a MetadataRegistry, instances: &'a mut InstanceMap) -> Self {
    Self {
      metadata,
      instances,
    }
  }

  pub fn instantiate<T>(
    &mut self,
    metadata: &str,
    name: &str,
    instantiate: impl Fn(Arc<T::M>) -> anyhow::Result<T>,
  ) -> anyhow::Result<&mut T>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    let metadata_item = if name.contains('@') {
      self.metadata.lookup::<T>(metadata, name)
    } else {
      self.metadata.lookup_in_any_group::<T>(metadata, name)
    }
    .ok_or(CoreError::MetadataNotFound(metadata.to_string()))?;

    let entry = self.instances.entry(full_name).or_default();

    let instance = instantiate(metadata_item)?;

    entry.push(Box::new(instance));

    let last = entry
      .last_mut()
      .expect("instance entry must contain one item");

    Ok(last.downcast_mut().expect("instance type mismatch"))
  }

  pub fn instantiate_one<T>(
    &mut self,
    metadata: &str,
    name: &str,
    instantiate: impl Fn(Arc<T::M>) -> anyhow::Result<T>,
  ) -> anyhow::Result<&mut T>
  where
    T: Model + 'static,
  {
    let full_name = format!("{metadata}@{name}");
    let insts = self.instances.get(&full_name);
    if let None = insts {
      self.instantiate(metadata, name, instantiate)
    } else if insts.unwrap().len() == 0 {
      self.instantiate(metadata, name, instantiate)
    } else {
      Ok(
        self
          .instances
          .get_mut(&full_name)
          .expect("instance entry exist")
          .first_mut()
          .expect("instance entry must contain one item")
          .downcast_mut::<T>()
          .expect("instance type mismatch"),
      )
    }
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
        .map(|x| x.downcast_ref::<Box<T>>().expect("instance type mismatch"))
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
        .map(|x| x.downcast_mut::<Box<T>>().expect("instance type mismatch"))
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

    Ok(
      instances
        .last()
        .expect("instance entry unexpectedly empty")
        .downcast_ref::<Box<T>>()
        .expect("instance type mismatch"),
    )
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
        .expect("instance entry unexpectedly empty")
        .downcast_mut::<Box<T>>()
        .expect("instance type mismatch"),
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
    let mut metadata = Metadata::new("units")
      .of::<Service>("service")
      .of::<Mount>("mount");

    let mut registry = MetadataRegistry::default();

    let src = r#"
[[service]]
name = "web"
run = "/bin/webd"

[[service]]
name = "api"
"#;

    registry
      .load_group_from_toml(&mut metadata, "demo", src)
      .expect("group should parse");
    registry.insert_metadata(metadata);

    let web = registry
      .find::<Service>("units", "demo@web")
      .expect("service should be indexed by unit@item");
    assert_eq!(web.name, "web");
    assert_eq!(web.run.as_deref(), Some("/bin/webd"));

    assert!(registry.find::<Service>("units", "demo@missing").is_none());
    assert!(registry.find::<Service>("units", "missing@web").is_none());
  }
}
