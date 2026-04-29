use std::{
  collections::{HashMap, HashSet},
  path::Path,
  sync::{Arc, Mutex, RwLock},
};

use once_cell::sync::Lazy;

use crate::{
  error::CoreError,
  types::{ToUstr, Ustr},
  user::UserStoreShared,
};

static PERM_REGISTRY: Lazy<Mutex<HashMap<u16, Ustr>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static NAME_REGISTRY: Lazy<Mutex<HashMap<Ustr, u16>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub trait PermissionChecker<T> {
  fn check(&self, store: &PermissionStore, item: T) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionId(pub u16);

impl PermissionId {
  pub fn new(name: impl Into<Ustr>, id: u16) -> Result<Self, CoreError> {
    let name = name.into();
    let mut reg = PERM_REGISTRY.lock().map_err(CoreError::custom)?;
    let mut regn = NAME_REGISTRY.lock().map_err(CoreError::custom)?;

    if let Some(name) = reg.get(&id) {
      return Err(CoreError::DuplicatePermissions {
        id,
        name: name.to_string(),
      });
    }

    reg.insert(id, name.clone());
    regn.insert(name, id);

    Ok(Self(id))
  }
}

impl From<u16> for PermissionId {
  fn from(value: u16) -> Self {
    Self(value)
  }
}

#[derive(Default)]
pub struct PermissionStoreInner {
  overlay_uid_grants: HashMap<u32, HashSet<u16>>,
  overlay_uid_revokes: HashMap<u32, HashSet<u16>>,
  overlay_gid_grants: HashMap<u32, HashSet<u16>>,
  overlay_gid_revokes: HashMap<u32, HashSet<u16>>,

  links: HashMap<u16, HashSet<u16>>,
  groups: HashMap<u16, Ustr>,
}

#[derive(Default, Clone)]
pub enum PermissionExpr {
  #[default]
  All,

  Any(Vec<PermissionExpr>),
  Exact(Vec<PermissionExpr>),

  Group(Ustr),
  Perm(PermissionId),
}

impl From<PermissionId> for PermissionExpr {
  fn from(value: PermissionId) -> Self {
    Self::Perm(value)
  }
}

impl From<String> for PermissionExpr {
  fn from(value: String) -> Self {
    Self::Group(value.to_ustr())
  }
}

impl From<Ustr> for PermissionExpr {
  fn from(value: Ustr) -> Self {
    Self::Group(value)
  }
}

#[derive(Default, Clone)]
pub struct PermissionStore {
  inner: Arc<RwLock<PermissionStoreInner>>,
  pub users: UserStoreShared,
}

impl PermissionStore {
  pub const KEY: &str = "runtime@permission_store";

  pub fn new(users: UserStoreShared) -> Self {
    Self {
      users,
      ..Default::default()
    }
  }

  pub fn user_has(&self, uid: u32, perm: PermissionId) -> bool {
    // should this be?
    if uid == 0 || perm.0 == 0 {
      return true;
    }

    let inner = self.inner.read().expect("permission store lock");

    let Some(user) = self.users.lookup_by_uid(uid) else {
      return false;
    };

    let groups = self.users.groups_for(user);

    if let Some(gs) = inner.groups.get(&perm.0) {
      if groups.contains(gs) {
        return true;
      }
    }

    let revoked = inner
      .overlay_uid_revokes
      .get(&uid)
      .map(|x| x.contains(&perm.0))
      .unwrap_or(false);
    if revoked {
      return false;
    }

    (user.permissions.contains(&perm.0)
      || inner
        .overlay_uid_grants
        .get(&uid)
        .map(|x| x.contains(&perm.0))
        .unwrap_or(false)
      || groups.iter().any(|x| {
        self
          .users
          .group_by_name(x)
          .map(|x| self.group_has(x.gid, perm))
          .unwrap_or(false)
      }))
      || inner.links.get(&perm.0).cloned().map_or(false, |x| {
        drop(inner);
        x.iter().any(|x| self.user_has(uid, (*x).into()))
      })
  }

  pub fn user_check(&self, uid: u32, expr: &PermissionExpr) -> bool {
    if uid == 0 {
      // short circuit?
      return true;
    }

    let Some(user) = self.users.lookup_by_uid(uid) else {
      return false;
    };

    let groups = self.users.groups_for(user);

    self.eval_expr(uid, expr, &groups)
  }

  fn eval_expr(&self, uid: u32, expr: &PermissionExpr, groups: &Vec<Ustr>) -> bool {
    match expr {
      PermissionExpr::All => true,

      PermissionExpr::Perm(p) => self.user_has(uid, *p),

      PermissionExpr::Group(name) => groups.iter().any(|g| g == name),

      PermissionExpr::Any(exprs) => exprs.iter().any(|e| self.eval_expr(uid, e, groups)),

      PermissionExpr::Exact(exprs) => exprs.iter().all(|e| self.eval_expr(uid, e, groups)),
    }
  }

  pub fn from_name(&self, name: &Ustr) -> Option<PermissionId> {
    if name.as_str() == "any" {
      return Some(PermissionId(0));
    }
    let regn = NAME_REGISTRY.lock().expect("permission store lock");
    regn.get(name).map(|x| PermissionId(*x))
  }

  pub fn group_has(&self, gid: u32, perm: PermissionId) -> bool {
    let inner = self.inner.read().expect("permission store lock");

    let Some(group) = self.users.group(gid) else {
      return false;
    };

    let revoked = inner
      .overlay_gid_revokes
      .get(&gid)
      .map(|x| x.contains(&perm.0))
      .unwrap_or(false);
    if revoked {
      return false;
    }

    (group.permissions.contains(&perm.0)
      || inner
        .overlay_gid_grants
        .get(&gid)
        .map(|x| x.contains(&perm.0))
        .unwrap_or(false))
      || inner.links.get(&perm.0).cloned().map_or(false, |x| {
        drop(inner);
        x.iter().any(|x| self.group_has(gid, (*x).into()))
      })
  }

  pub fn link(&self, perm: u16, parent: u16) {
    let mut inner = self.inner.write().expect("permission store lock");

    if let Some(k) = inner.links.get(&parent) {
      if k.contains(&perm) {
        return;
      }
    }

    inner.links.entry(perm).or_default().insert(parent);
  }

  pub fn or_group(&self, perm: u16, group: Ustr) {
    let mut inner = self.inner.write().expect("permission store lock");

    inner.groups.insert(perm, group);
  }

  pub fn grant_user(&self, uid: u32, perm: PermissionId) {
    let mut inner = self.inner.write().expect("permission store lock");
    inner
      .overlay_uid_revokes
      .entry(uid)
      .or_default()
      .remove(&perm.0);
    inner
      .overlay_uid_grants
      .entry(uid)
      .or_default()
      .insert(perm.0);
  }

  pub fn grant_group(&self, gid: u32, perm: PermissionId) {
    let mut inner = self.inner.write().expect("permission store lock");
    inner
      .overlay_gid_revokes
      .entry(gid)
      .or_default()
      .remove(&perm.0);
    inner
      .overlay_gid_grants
      .entry(gid)
      .or_default()
      .insert(perm.0);
  }

  pub fn ungrant_user(&self, uid: u32, perm: PermissionId) {
    let mut inner = self.inner.write().expect("permission store lock");
    inner
      .overlay_uid_grants
      .entry(uid)
      .or_default()
      .remove(&perm.0);
    inner
      .overlay_uid_revokes
      .entry(uid)
      .or_default()
      .insert(perm.0);
  }

  pub fn ungrant_group(&self, gid: u32, perm: PermissionId) {
    let mut inner = self.inner.write().expect("permission store lock");
    inner
      .overlay_gid_grants
      .entry(gid)
      .or_default()
      .remove(&perm.0);
    inner
      .overlay_gid_revokes
      .entry(gid)
      .or_default()
      .insert(perm.0);
  }

  pub fn write_perms_with_overlay(&self, perms_path: &Path) -> Result<(), CoreError> {
    let inner = self.inner.read().expect("permission store lock");

    self.users.write_perms_with_overlay(
      perms_path,
      &inner.overlay_uid_grants,
      &inner.overlay_uid_revokes,
      &inner.overlay_gid_grants,
      &inner.overlay_gid_revokes,
    )
  }

  pub fn new_perm(&self, name: impl Into<Ustr>, id: u16) -> Result<PermissionId, CoreError> {
    PermissionId::new(name, id)
  }

  pub fn reg_perm(&self, perm: PermissionId, name: impl Into<Ustr>) -> Result<&Self, CoreError> {
    let id = perm.0;
    let mut reg = PERM_REGISTRY.lock().map_err(CoreError::custom)?;

    if let Some(name) = reg.get(&id) {
      return Err(CoreError::DuplicatePermissions {
        id,
        name: name.to_string(),
      });
    }

    reg.insert(id, name.into());

    Ok(self)
  }
}
