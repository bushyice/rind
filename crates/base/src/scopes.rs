use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rind_core::types::Ustr;

#[derive(Debug, Clone, Default)]
pub struct ScopeInfo {
  pub name: Ustr,
  pub attributes: HashMap<Ustr, String>,
  pub lifetime_state: Option<Ustr>,
}

impl ScopeInfo {
  pub fn user(&self) -> Option<Ustr> {
    self.attributes.get(&Ustr::from("user")).map(Ustr::from)
  }
}

#[derive(Debug, Default, Clone)]
pub struct ScopeStore {
  scopes: HashMap<Ustr, ScopeInfo>,
  state_to_scope: HashMap<Ustr, Ustr>,
}

pub static GLOBAL_SCOPE_STORE: LazyLock<Mutex<ScopeStore>> =
  LazyLock::new(|| Mutex::new(ScopeStore::default()));
pub static GLOBAL_SCOPE_SPECS: LazyLock<Mutex<HashMap<Ustr, ScopeInfo>>> =
  LazyLock::new(|| Mutex::new(HashMap::new()));

impl ScopeStore {
  pub const KEY: &str = "runtime:scope_store";

  pub fn upsert(
    &mut self,
    name: impl Into<Ustr>,
    attributes: HashMap<Ustr, String>,
    lifetime_state: Option<Ustr>,
  ) {
    let name = name.into();

    // TODO: Remove old state_to_scope mapping if the scope exists with a different state
    if let Some(existing) = self.scopes.get(&name) {
      if let Some(old_state) = &existing.lifetime_state {
        if Some(old_state) != lifetime_state.as_ref() {
          self.state_to_scope.remove(old_state);
        }
      }
    }

    if let Some(state_name) = lifetime_state.clone() {
      self.state_to_scope.insert(state_name, name.clone());
    }
    self.scopes.insert(
      name.clone(),
      ScopeInfo {
        name,
        attributes,
        lifetime_state,
      },
    );
  }

  pub fn by_name(&self, scope: &str) -> Option<&ScopeInfo> {
    self.scopes.get(&Ustr::from(scope))
  }

  pub fn remove_scope(&mut self, scope: &str) -> bool {
    let Some(info) = self.scopes.remove(&Ustr::from(scope)) else {
      return false;
    };
    if let Some(state_name) = &info.lifetime_state {
      self.state_to_scope.remove(state_name);
    }
    true
  }

  pub fn scope_for_state(&self, state_name: &str) -> Option<Ustr> {
    self.state_to_scope.get(&Ustr::from(state_name)).cloned()
  }

  pub fn list(&self) -> Vec<ScopeInfo> {
    self.scopes.values().cloned().collect()
  }

  pub fn has_global(name: impl Into<Ustr>) -> bool {
    let store = GLOBAL_SCOPE_STORE
      .lock()
      .expect("scope store lock poisoned");
    store.scopes.contains_key(&name.into())
  }

  pub fn upsert_global(
    name: impl Into<Ustr>,
    attributes: HashMap<Ustr, String>,
    lifetime_state: Option<Ustr>,
  ) {
    let mut store = GLOBAL_SCOPE_STORE
      .lock()
      .expect("scope store lock poisoned");
    store.upsert(name, attributes, lifetime_state);
  }

  pub fn remove_scope_global(scope: &str) -> bool {
    let mut store = GLOBAL_SCOPE_STORE
      .lock()
      .expect("scope store lock poisoned");
    store.remove_scope(scope)
  }

  pub fn user_for_scope(scope: &str) -> Option<Ustr> {
    let store = GLOBAL_SCOPE_STORE
      .lock()
      .expect("scope store lock poisoned");
    store.by_name(scope).and_then(|s| s.user())
  }

  pub fn list_global() -> Vec<ScopeInfo> {
    let store = GLOBAL_SCOPE_STORE
      .lock()
      .expect("scope store lock poisoned");
    store.list()
  }

  pub fn desired_scope_upsert(
    name: impl Into<Ustr>,
    attributes: HashMap<Ustr, String>,
    lifetime_state: Option<Ustr>,
  ) {
    let name = name.into();
    let mut specs = GLOBAL_SCOPE_SPECS
      .lock()
      .expect("scope specs lock poisoned");
    specs.insert(
      name.clone(),
      ScopeInfo {
        name,
        attributes,
        lifetime_state,
      },
    );
  }

  pub fn desired_scope_remove(name: &str) {
    let mut specs = GLOBAL_SCOPE_SPECS
      .lock()
      .expect("scope specs lock poisoned");
    specs.remove(&Ustr::from(name));
  }

  pub fn desired_scopes() -> Vec<ScopeInfo> {
    let specs = GLOBAL_SCOPE_SPECS
      .lock()
      .expect("scope specs lock poisoned");
    specs.values().cloned().collect()
  }
}
