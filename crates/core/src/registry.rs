use std::{
  any::{Any, TypeId},
  collections::HashMap,
  sync::Arc,
};

use crate::{
  error::{CoreError, CoreResult},
  metadata::{Metadata, Model, NamedItem},
  rslvns,
  types::ToUstr,
  utils::parse_scoped_name,
};

use crate::types::Ustr;

#[derive(Default)]
pub struct MetadataRegistry {
  metadata: HashMap<Ustr, Arc<Metadata>>,
  pub indexes: HashMap<TypeId, HashMap<Ustr, usize>>,
}

impl MetadataRegistry {
  pub(crate) fn resolve_metadata_key(&self, metadata: Ustr, full_name: &str) -> Ustr {
    if metadata.as_str() == "*" {
      let scope = parse_scoped_name(full_name).scope;
      if self.metadata.contains_key(&scope) {
        return scope;
      }
      let units = Ustr::from("units");
      if self.metadata.contains_key(&units) {
        return units;
      }
      let statik = Ustr::from("static");
      if self.metadata.contains_key(&statik) {
        return statik;
      }
      if let Some(first) = self.metadata.keys().next() {
        return first.clone();
      }
    }

    if self.metadata.contains_key(&metadata) {
      return metadata;
    }

    let scope = parse_scoped_name(full_name).scope;
    if self.metadata.contains_key(&scope) {
      scope
    } else {
      metadata
    }
  }

  pub fn insert_metadata(&mut self, metadata: Metadata) {
    let name = metadata.name.clone();
    self.metadata.insert(name, Arc::new(metadata));
  }

  pub fn load_group_from_toml(
    &mut self,
    metadata: &mut Metadata,
    group: impl Into<Ustr>,
    source: &str,
  ) -> anyhow::Result<()> {
    metadata.from_toml(source, group)?;
    self.indexes.clear();
    Ok(())
  }

  pub fn group_items<T: Model + 'static>(
    &self,
    metadata: impl Into<Ustr>,
    group: impl Into<Ustr>,
  ) -> Option<Vec<Arc<T::M>>> {
    let metadata = metadata.into();
    let group = group.into();
    self
      .metadata
      .get(&metadata)?
      .get_in_group::<T>(group)
      .map(|x| x.iter().map(|x| x.clone()).collect())
  }

  pub fn items<T: Model + 'static>(
    &self,
    metadata: impl Into<Ustr>,
  ) -> Option<Vec<(Ustr, Arc<T::M>)>> {
    let metadata = metadata.into();
    if metadata.as_str() == "*" {
      let mut out = Vec::new();
      for m in self.metadata.values() {
        out.extend(m.groups().flat_map(|group| {
          m.get_in_group::<T>(group.clone())
            .into_iter()
            .flatten()
            .map(move |item| (group.clone(), item.clone()))
        }));
      }
      return Some(out);
    }
    let m = self.metadata.get(&metadata)?;

    Some(
      m.groups()
        .flat_map(|group| {
          m.get_in_group::<T>(group.clone())
            .into_iter()
            .flatten()
            .map(move |item| (group.clone(), item.clone()))
        })
        .collect(),
    )
  }

  pub fn all_groups(&self) -> HashMap<Ustr, Vec<Ustr>> {
    let mut out: HashMap<Ustr, Vec<Ustr>> = HashMap::new();
    for (u, m) in self.metadata.iter() {
      out.entry(u.clone()).or_default().extend(m.groups());
    }
    out
  }

  pub fn groups(&self, metadata: impl Into<Ustr>) -> Option<Vec<Ustr>> {
    let metadata = metadata.into();
    let m = self.metadata.get(&metadata)?;

    Some(m.groups().collect())
  }

  pub fn ensure_index_for_type<T>(&mut self, metadata: impl Into<Ustr>) -> anyhow::Result<()>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let type_id = TypeId::of::<T>();
    if self.indexes.contains_key(&type_id) {
      return Ok(());
    }

    let m = self
      .metadata
      .get_mut(&metadata)
      .ok_or(CoreError::MetadataNotFound(metadata.to_string()))?;

    let mut map = HashMap::new();
    for group in m.groups() {
      if let Some(items) = m.get_in_group::<T>(group.clone()) {
        for (idx, item) in items.iter().enumerate() {
          map.insert(Ustr::from(rslvns!(group, item.name())), idx);
        }
      }
    }

    self.indexes.insert(type_id, map);

    Ok(())
  }

  pub fn find<T>(&self, metadata: impl Into<Ustr>, full_name: impl Into<Ustr>) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let full_name = full_name.into();
    let metadata = self.resolve_metadata_key(metadata, full_name.as_str());
    let (group, _, _) = rslvns!(res full_name); //full_name.as_str().split_once(':')?;

    if let Some(idx) = self.indexes.get(&TypeId::of::<T>())?.get(&full_name) {
      self
        .group_items::<T>(metadata, group)?
        .get(*idx)
        .map(|x| x.clone())
    } else {
      self.lookup::<T>(metadata, full_name)
    }
  }

  pub fn lookup<T>(
    &self,
    metadata: impl Into<Ustr>,
    full_name: impl Into<Ustr>,
  ) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let full_name = full_name.into();
    let metadata = self.resolve_metadata_key(metadata, full_name.as_str());
    let (group, item_name, _) = rslvns!(res full_name);
    self
      .group_items::<T>(metadata, group)?
      .iter()
      .find(|item| item.name() == item_name)
      .map(|x| x.clone())
  }

  pub fn lookup_in_any_group<T>(
    &self,
    metadata: impl Into<Ustr>,
    item_name: &str,
  ) -> Option<Arc<T::M>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let m = self.metadata.get(&metadata)?;
    for group in m.groups() {
      let full = rslvns!(group, item_name); //format!("{group}:{item_name}");
      if let Some(found) = self.lookup::<T>(metadata.clone(), full.as_str()) {
        return Some(found);
      }
    }
    None
  }

  pub fn metadata(&self, metadata: impl Into<Ustr>) -> Option<Arc<Metadata>> {
    self.metadata.get(&metadata.into()).map(|x| x.clone())
  }

  pub fn metadata_names(&self) -> impl Iterator<Item = Ustr> + '_ {
    self.metadata.keys().cloned()
  }

  pub fn remove_metadata(&mut self, metadata: impl Into<Ustr>) -> bool {
    let metadata = metadata.into();
    let removed = self.metadata.remove(&metadata).is_some();
    if removed {
      self.indexes.clear();
    }
    removed
  }
}

pub type InstanceMap = HashMap<Ustr, Vec<Box<dyn Any>>>;

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
  fn resolved_metadata_and_full_name(&self, metadata: Ustr, name: Ustr) -> (Ustr, Ustr) {
    let resolved = self.metadata.resolve_metadata_key(metadata, name.as_str());
    (resolved.clone(), rslvns!(scp resolved, name).to_ustr())
  }

  pub fn new(metadata: &'a MetadataRegistry, instances: &'a mut InstanceMap) -> Self {
    Self {
      metadata,
      instances,
    }
  }

  pub fn instantiate<T>(
    &mut self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
    mut instantiate: impl FnMut(Arc<T::M>) -> CoreResult<T>,
  ) -> CoreResult<&mut T>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (metadata, full_name) = self.resolved_metadata_and_full_name(metadata, name.clone());
    let metadata_item = if name.as_str().contains(':') {
      self.metadata.find::<T>(metadata.clone(), name.clone())
    } else {
      self
        .metadata
        .lookup_in_any_group::<T>(metadata.clone(), name.as_str())
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
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
    instantiate: impl FnMut(Arc<T::M>) -> CoreResult<T>,
  ) -> CoreResult<&mut T>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (metadata, full_name) = self.resolved_metadata_and_full_name(metadata, name.clone());
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

  pub fn instances<T>(
    &self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
  ) -> anyhow::Result<Vec<&T>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (_, full_name) = self.resolved_metadata_and_full_name(metadata, name);
    Ok(
      self
        .instances
        .get(&full_name)
        .ok_or(CoreError::MissingInstances(full_name.to_string()))?
        .iter()
        .map(|x| x.downcast_ref::<T>().expect("instance type mismatch"))
        .collect(),
    )
  }

  pub fn uninstantiate<T>(
    &mut self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
  ) -> anyhow::Result<Vec<Box<T>>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (_, full_name) = self.resolved_metadata_and_full_name(metadata, name);
    Ok(
      self
        .instances
        .remove(&full_name)
        .ok_or(CoreError::MissingInstances(full_name.to_string()))?
        .into_iter()
        .map(|x| x.downcast::<T>().expect("instance type mismatch"))
        .collect(),
    )
  }

  pub fn uninstantiate_one<T>(
    &mut self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
  ) -> anyhow::Result<Box<T>>
  where
    T: Model + 'static,
  {
    self
      .uninstantiate::<T>(metadata, name)
      .map(|mut x| x.pop().expect("instance entry unexpectedly empty"))
  }

  pub fn instances_mut<T>(
    &mut self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
  ) -> anyhow::Result<Vec<&mut T>>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (_, full_name) = self.resolved_metadata_and_full_name(metadata, name);
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

  pub fn as_one<T>(&self, metadata: impl Into<Ustr>, name: impl Into<Ustr>) -> anyhow::Result<&T>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (_, full_name) = self.resolved_metadata_and_full_name(metadata, name);
    let instances = self
      .instances
      .get(&full_name)
      .ok_or(CoreError::MissingInstances(full_name.to_string()))?;

    Ok(
      instances
        .last()
        .expect("instance entry unexpectedly empty")
        .downcast_ref::<T>()
        .expect("instance type mismatch"),
    )
  }

  pub fn as_one_mut<T>(
    &mut self,
    metadata: impl Into<Ustr>,
    name: impl Into<Ustr>,
  ) -> anyhow::Result<&mut T>
  where
    T: Model + 'static,
  {
    let metadata = metadata.into();
    let name = name.into();
    let (_, full_name) = self.resolved_metadata_and_full_name(metadata, name);
    let instances = self
      .instances
      .get_mut(&full_name)
      .ok_or(CoreError::MissingInstances(full_name.to_string()))?;

    Ok(
      instances
        .last_mut()
        .expect("instance entry unexpectedly empty")
        .downcast_mut::<T>()
        .expect("instance type mismatch"),
    )
  }

  pub fn singleton<T: 'static>(&self, key: impl Into<Ustr>) -> Option<&T> {
    self
      .instances
      .get(&key.into())?
      .first()?
      .downcast_ref::<T>()
  }

  pub fn singleton_mut<T: 'static>(&mut self, key: impl Into<Ustr>) -> Option<&mut T> {
    self
      .instances
      .get_mut(&key.into())?
      .first_mut()?
      .downcast_mut::<T>()
  }

  pub fn singleton_or_insert_with<T: 'static>(
    &mut self,
    key: impl Into<Ustr>,
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
  (@string $T:ident) => { $crate::types::Ustr };
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
              .ok_or(CoreError::MissingField { path: $k.to_string() })?;
            ($k, val)
          };
        )+

        let result = {
          let tuple = (
            $(
              {
                let val = $k.1
                  .first_mut()
                  .ok_or(CoreError::MissingField { path: $k.0.to_string() })?;

                let downcasted = val
                  .downcast_mut::<$T>()
                  .ok_or(CoreError::TypeMismatch {
                    path: $k.0.to_string(),
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
    name: Ustr,
    run: Option<Ustr>,
  }

  #[model(meta_name = target, meta_fields(target))]
  struct Mount {
    target: Ustr,
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
      .find::<Service>("units", rslvns!("demo", "web"))
      .expect("service should be indexed by unit:item");
    assert_eq!(web.name.as_str(), "web");
    assert_eq!(web.run.as_ref().map(|x| x.as_str()), Some("/bin/webd"));

    assert!(
      registry
        .find::<Service>("units", rslvns!("demo", "missing"))
        .is_none()
    );
    assert!(
      registry
        .find::<Service>("units", rslvns!("missing", "web"))
        .is_none()
    );
  }
}
