use std::{
  collections::{HashMap, HashSet},
  path::Path,
  sync::{Arc, RwLock},
};

use crate::{error::CoreError, user::UserStoreShared};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionId(u16);

impl From<u16> for PermissionId {
  fn from(value: u16) -> Self {
    Self(value)
  }
}

#[derive(Default)]
pub struct PermissionStoreInner {
  permissions: HashMap<PermissionId, String>,
}

#[derive(Default, Clone)]
pub struct PermissionStore {
  inner: Arc<RwLock<PermissionStoreInner>>,
  users: UserStoreShared,
  overlay_uid_grants: HashMap<u32, HashSet<u16>>,
  overlay_uid_revokes: HashMap<u32, HashSet<u16>>,
  overlay_gid_grants: HashMap<u32, HashSet<u16>>,
  overlay_gid_revokes: HashMap<u32, HashSet<u16>>,
}

impl PermissionStore {
  pub fn register(&mut self, name: impl Into<String>, permid: impl Into<PermissionId>) {
    let permid = permid.into();
    let mut inner = self.inner.write().expect("permission store lock");
    inner.permissions.insert(permid, name.into());
  }

  pub fn user_has(&self, uid: u32, perm: PermissionId) -> bool {
    let Some(user) = self.users.lookup_by_uid(uid) else {
      return false;
    };

    let revoked = self
      .overlay_uid_revokes
      .get(&uid)
      .map(|x| x.contains(&perm.0))
      .unwrap_or(false);
    if revoked {
      return false;
    }

    user.permissions.contains(&perm.0)
      || self
        .overlay_uid_grants
        .get(&uid)
        .map(|x| x.contains(&perm.0))
        .unwrap_or(false)
  }

  pub fn group_has(&self, gid: u32, perm: PermissionId) -> bool {
    let Some(group) = self.users.group(gid) else {
      return false;
    };

    let revoked = self
      .overlay_gid_revokes
      .get(&gid)
      .map(|x| x.contains(&perm.0))
      .unwrap_or(false);
    if revoked {
      return false;
    }

    group.permissions.contains(&perm.0)
      || self
        .overlay_gid_grants
        .get(&gid)
        .map(|x| x.contains(&perm.0))
        .unwrap_or(false)
  }

  pub fn grant_user(&mut self, uid: u32, perm: PermissionId) {
    self
      .overlay_uid_revokes
      .entry(uid)
      .or_default()
      .remove(&perm.0);
    self.overlay_uid_grants.entry(uid).or_default().insert(perm.0);
  }

  pub fn grant_group(&mut self, gid: u32, perm: PermissionId) {
    self
      .overlay_gid_revokes
      .entry(gid)
      .or_default()
      .remove(&perm.0);
    self.overlay_gid_grants.entry(gid).or_default().insert(perm.0);
  }

  pub fn ungrant_user(&mut self, uid: u32, perm: PermissionId) {
    self
      .overlay_uid_grants
      .entry(uid)
      .or_default()
      .remove(&perm.0);
    self.overlay_uid_revokes.entry(uid).or_default().insert(perm.0);
  }

  pub fn ungrant_group(&mut self, gid: u32, perm: PermissionId) {
    self
      .overlay_gid_grants
      .entry(gid)
      .or_default()
      .remove(&perm.0);
    self.overlay_gid_revokes.entry(gid).or_default().insert(perm.0);
  }

  pub fn write_perms_with_overlay(&self, perms_path: &Path) -> Result<(), CoreError> {
    self.users.write_perms_with_overlay(
      perms_path,
      &self.overlay_uid_grants,
      &self.overlay_uid_revokes,
      &self.overlay_gid_grants,
      &self.overlay_gid_revokes,
    )
  }
}
