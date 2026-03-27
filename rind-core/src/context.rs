use std::{
  any::{Any, TypeId},
  collections::{HashMap, HashSet},
  sync::{Arc, RwLock},
  time::Instant,
};

use crate::user::UserRecord;

use crate::registry::InstanceRegistry;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeId(String);

impl RuntimeId {
  pub fn as_str(&self) -> &str {
    self.0.as_str()
  }
}

impl From<String> for RuntimeId {
  fn from(value: String) -> Self {
    Self(value)
  }
}

impl From<&str> for RuntimeId {
  fn from(value: &str) -> Self {
    Self(value.to_string())
  }
}

#[derive(Default)]
pub struct RuntimeScope {
  values: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl RuntimeScope {
  pub fn insert<T>(&mut self, value: T)
  where
    T: Send + Sync + 'static,
  {
    self.values.insert(TypeId::of::<T>(), Box::new(value));
  }

  pub fn get<T>(&self) -> Option<&T>
  where
    T: Send + Sync + 'static,
  {
    self.values.get(&TypeId::of::<T>())?.downcast_ref::<T>()
  }

  pub fn get_mut<T>(&mut self) -> Option<&mut T>
  where
    T: Send + Sync + 'static,
  {
    self.values.get_mut(&TypeId::of::<T>())?.downcast_mut::<T>()
  }

  pub fn extend(&mut self, other: RuntimeScope) {
    self.values.extend(other.values);
  }
}

#[derive(Default)]
pub struct RuntimeScopes {
  scopes: HashMap<RuntimeId, RuntimeScope>,
  globals: Vec<Box<dyn Fn(&mut RuntimeScope) -> ()>>,
  globals_applied: HashSet<RuntimeId>,
}

impl RuntimeScopes {
  pub fn insert<T>(&mut self, runtime_id: impl Into<RuntimeId>, value: T)
  where
    T: Send + Sync + 'static,
  {
    let runtime_id = runtime_id.into();
    self.scopes.entry(runtime_id).or_default().insert(value);
  }

  pub fn scope(&self, runtime_id: &str) -> Option<&RuntimeScope> {
    self.scopes.get(&RuntimeId::from(runtime_id))
  }

  pub fn scope_mut(&mut self, runtime_id: &str) -> Option<&mut RuntimeScope> {
    self.scopes.get_mut(&RuntimeId::from(runtime_id))
  }

  pub fn take_scope(&mut self, runtime_id: &str) -> Option<RuntimeScope> {
    self.scopes.remove(&RuntimeId::from(runtime_id))
  }

  pub fn take_or_build_scope(&mut self, runtime_id: impl Into<RuntimeId>) -> RuntimeScope {
    let runtime_id = runtime_id.into();
    let mut scope = self.scopes.remove(&runtime_id).unwrap_or_default();

    if self.globals_applied.insert(runtime_id) {
      for global in &self.globals {
        global(&mut scope);
      }
    }

    scope
  }

  pub fn put_scope(&mut self, runtime_id: impl Into<RuntimeId>, scope: RuntimeScope) {
    self.scopes.insert(runtime_id.into(), scope);
  }

  pub fn insert_scope(&mut self, runtime_id: impl Into<RuntimeId>, scope: RuntimeScope) {
    let runtime_id = runtime_id.into();
    self.scopes.entry(runtime_id).or_default().extend(scope);
  }

  pub fn insert_globals(&self, scope: &mut RuntimeScope) {
    for global in self.globals.iter() {
      global(scope);
    }
  }
}

#[derive(Default)]
pub struct ScopeBuilder {
  scopes: RuntimeScopes,
}

impl ScopeBuilder {
  pub fn insert<T>(&mut self, runtime_id: impl Into<RuntimeId>, build: impl FnOnce() -> T)
  where
    T: Send + Sync + 'static,
  {
    self.scopes.insert(runtime_id, build());
  }

  pub fn build(self) -> RuntimeScopes {
    self.scopes
  }

  pub fn globals(&mut self, definer: impl Fn(&mut RuntimeScope) + 'static) {
    self.scopes.globals.push(Box::new(definer));
  }

  pub fn insert_scope(
    &mut self,
    runtime_id: impl Into<RuntimeId>,
    build: impl FnOnce() -> RuntimeScope,
  ) {
    self.scopes.insert_scope(runtime_id, build());
  }
}

#[derive(Debug, Clone)]
pub struct UserContext {
  pub record: UserRecord,
  pub groups: Vec<String>,
}

impl UserContext {
  pub fn new(record: UserRecord, groups: Vec<String>) -> Self {
    Self { record, groups }
  }

  pub fn in_group(&self, group: &str) -> bool {
    self.groups.iter().any(|g| g == group)
  }

  pub fn is_root(&self) -> bool {
    self.record.uid == 0
  }

  pub fn is_privileged(&self) -> bool {
    self.is_root() || self.in_group("wheel") || self.in_group("sudo")
  }
}

#[derive(Debug, Clone)]
pub struct UserSession {
  pub id: u64,
  pub user: UserContext,
  pub tty: String,
  pub started_at: Instant,
}

pub type UserSessionStore = Arc<RwLock<HashMap<u64, UserSession>>>;

pub fn new_session_store() -> UserSessionStore {
  Arc::new(RwLock::new(HashMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSpace {
  System,
  User(u32),
}

pub struct RuntimeContext<'a> {
  pub runtime_id: &'a str,
  pub scope: &'a mut RuntimeScope,
  pub registry: InstanceRegistry<'a>,
  pub space: RuntimeSpace,
}

impl<'a> RuntimeContext<'a> {
  pub fn new(
    runtime_id: &'a str,
    scope: &'a mut RuntimeScope,
    registry: InstanceRegistry<'a>,
  ) -> Self {
    Self {
      runtime_id,
      scope,
      registry,
      space: RuntimeSpace::System,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::{AtomicUsize, Ordering};

  #[derive(Debug, Default, PartialEq, Eq)]
  struct MyType {
    value: usize,
  }

  #[test]
  fn scope_builder_inserts_per_runtime() {
    let mut builder = ScopeBuilder::default();
    builder.insert("svc", || MyType { value: 42 });

    let scopes = builder.build();
    let value = scopes
      .scope("svc")
      .and_then(|scope| scope.get::<MyType>())
      .expect("typed runtime value should exist");

    assert_eq!(*value, MyType { value: 42 });
    assert!(scopes.scope("missing").is_none());
  }

  #[test]
  fn scope_builder_inserts_runtime_scope_values() {
    let mut builder = ScopeBuilder::default();
    builder.insert_scope("svc", || {
      let mut scope = RuntimeScope::default();
      scope.insert(MyType { value: 7 });
      scope
    });

    let scopes = builder.build();
    let value = scopes
      .scope("svc")
      .and_then(|scope| scope.get::<MyType>())
      .expect("typed runtime value from runtime scope should exist");

    assert_eq!(*value, MyType { value: 7 });
  }

  #[test]
  fn take_or_build_scope_applies_globals_once_per_runtime() {
    let mut builder = ScopeBuilder::default();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    builder.globals(move |_scope| {
      counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    let mut scopes = builder.build();
    let scope = scopes.take_or_build_scope("svc");
    scopes.put_scope("svc", scope);
    let scope = scopes.take_or_build_scope("svc");
    scopes.put_scope("svc", scope);

    assert_eq!(counter.load(Ordering::SeqCst), 1);
  }
}
