use std::{
  any::{Any, TypeId},
  collections::HashMap,
};

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
}

#[derive(Default)]
pub struct RuntimeScopes {
  scopes: HashMap<RuntimeId, RuntimeScope>,
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
}

pub struct RuntimeContext<'a> {
  pub runtime_id: &'a str,
  pub scope: &'a RuntimeScope,
}

impl<'a> RuntimeContext<'a> {
  pub fn new(runtime_id: &'a str, scope: &'a RuntimeScope) -> Self {
    Self { runtime_id, scope }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

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
}
