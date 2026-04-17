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

  pub fn items<T: Model + 'static>(&self, metadata: &str) -> Option<Vec<(String, Arc<T::M>)>> {
    let m = self.metadata.get(metadata)?;

    Some(
      m.groups()
        .flat_map(|group| {
          m.get_in_group::<T>(group)
            .into_iter()
            .flatten()
            .map(move |item| (group.to_string(), item.clone()))
        })
        .collect(),
    )
  }

  pub fn groups(&self, metadata: &str) -> Option<Vec<&str>> {
    let m = self.metadata.get(metadata)?;

    Some(m.groups().collect())
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

  pub fn remove_metadata(&mut self, metadata: &str) -> bool {
    let removed = self.metadata.remove(metadata).is_some();
    if removed {
      self.indexes.clear();
    }
    removed
  }
}

pub type InstanceMap = HashMap<String, Vec<Box<dyn Any>>>;

pub struct InstanceRegistry<'a> {
  pub metadata: &'a MetadataRegistry,
  pub instances: &'a mut InstanceMap,
}

#[doc(hidden)]
pub trait HandleTuple: Sized {
  type Keys;
  fn run<R>(
    store: &mut InstanceRegistry,
    keys: Self::Keys,
    f: impl FnOnce(&mut InstanceRegistry, Self) -> Result<R, CoreError>,
  ) -> Result<R, CoreError>;
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

  pub fn instances<T>(&self, metadata: &str, name: &str) -> anyhow::Result<Vec<&T>>
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
        .map(|x| x.downcast_ref::<T>().expect("instance type mismatch"))
        .collect(),
    )
  }

  pub fn instances_mut<T>(&mut self, metadata: &str, name: &str) -> anyhow::Result<Vec<&mut T>>
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
        .map(|x| x.downcast_mut::<T>().expect("instance type mismatch"))
        .collect(),
    )
  }

  pub fn as_one<T>(&self, metadata: &str, name: &str) -> anyhow::Result<&T>
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
        .downcast_ref::<T>()
        .expect("instance type mismatch"),
    )
  }

  pub fn as_one_mut<T>(&mut self, metadata: &str, name: &str) -> anyhow::Result<&mut T>
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
        .downcast_mut::<T>()
        .expect("instance type mismatch"),
    )
  }

  pub fn singleton<T: 'static>(&self, key: &str) -> Option<&T> {
    self.instances.get(key)?.first()?.downcast_ref::<T>()
  }

  pub fn singleton_mut<T: 'static>(&mut self, key: &str) -> Option<&mut T> {
    self
      .instances
      .get_mut(key)?
      .first_mut()?
      .downcast_mut::<T>()
  }

  pub fn singleton_or_insert_with<T: 'static>(
    &mut self,
    key: impl Into<String>,
    init: impl FnOnce() -> T,
  ) -> &mut T {
    let key = key.into();
    let entry = self.instances.entry(key).or_default();
    if entry.is_empty() {
      entry.push(Box::new(init()));
    }
    entry
      .first_mut()
      .expect("singleton entry unexpectedly empty")
      .downcast_mut::<T>()
      .expect("singleton type mismatch")
  }

  #[allow(warnings)]
  pub fn singleton_handle<T, R>(
    &mut self,
    keys: T::Keys,
    f: impl FnOnce(&mut InstanceRegistry, T) -> Result<R, CoreError>,
  ) -> Result<R, CoreError>
  where
    T: HandleTuple,
  {
    T::run(self, keys, f)
  }
}

macro_rules! impl_handle_tuple {
  (@string $T:ident) => { String };
  ($($T:ident),+) => {
    impl_handle_tuple!($($T : $T),+);
  };
  ($($T:ident : $k:ident),+) => {
    impl<$($T: 'static),+> HandleTuple for ($(&mut $T,)+) {
      type Keys = ($(impl_handle_tuple!(@string $T),)+);

      fn run<R>(
        store: &mut InstanceRegistry,
        keys: Self::Keys,
        f: impl FnOnce(&mut InstanceRegistry, Self) -> Result<R, CoreError>,
      ) -> Result<R, CoreError> {
        #[allow(non_snake_case)]
        let ($($k,)+) = keys;

        {
          let keys_slice = [$( &$k ),+];
          for i in 0..keys_slice.len() {
            for j in i + 1..keys_slice.len() {
              if keys_slice[i] == keys_slice[j] {
                return Err(CoreError::DoubleKey);
              }
            }
          }
        }

        $(
          #[allow(non_snake_case)]
          let mut $k = {
            let val = store.instances
              .remove(&$k)
              .ok_or(CoreError::MissingField { path: $k.clone() })?;
            ($k, val)
          };
        )+

        let result = {
          let tuple = (
            $(
              {
                let val = $k.1
                  .first_mut()
                  .ok_or(CoreError::MissingField { path: $k.0.clone() })?;

                let downcasted = val
                  .downcast_mut::<$T>()
                  .ok_or(CoreError::TypeMismatch {
                    path: $k.0.clone(),
                    expected: stringify!($T).into(),
                  })?;

                unsafe { &mut *(downcasted as *mut $T) }
              },
            )+
          );

          f(store, tuple)
        };

        $(
          store.instances.insert($k.0, $k.1);
        )+

        result
      }
    }
  };
}

impl_handle_tuple!(A);
impl_handle_tuple!(A, B);
impl_handle_tuple!(A, B, C);
impl_handle_tuple!(A, B, C, D);
impl_handle_tuple!(A, B, C, D, E);
impl_handle_tuple!(A, B, C, D, E, F);

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
    registry
      .ensure_index_for_type::<Service>("units")
      .expect("index build should succeed");

    let web = registry
      .find::<Service>("units", "demo@web")
      .expect("service should be indexed by unit@item");
    assert_eq!(web.name, "web");
    assert_eq!(web.run.as_deref(), Some("/bin/webd"));

    assert!(registry.find::<Service>("units", "demo@missing").is_none());
    assert!(registry.find::<Service>("units", "missing@web").is_none());
  }
}
