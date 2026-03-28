use std::{
  collections::{HashMap, HashSet},
  path::Path,
  sync::{Arc, RwLock},
};

use crate::{error::CoreError, user::UserStoreShared};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionId(pub u16);

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
}

#[derive(Default, Clone)]
pub struct PermissionStore {
  inner: Arc<RwLock<PermissionStoreInner>>,
  users: UserStoreShared,
}

impl PermissionStore {
  pub fn new(users: UserStoreShared) -> Self {
    Self {
      users,
      ..Default::default()
    }
  }

  pub fn user_has(&self, uid: u32, perm: PermissionId) -> bool {
    // should this be?
    if uid == 0 {
      return true;
    }

    let inner = self.inner.read().expect("permission store lock");

    let Some(user) = self.users.lookup_by_uid(uid) else {
      return false;
    };

    let groups = self.users.groups_for(user);

    let revoked = inner
      .overlay_uid_revokes
      .get(&uid)
      .map(|x| x.contains(&perm.0))
      .unwrap_or(false);
    if revoked {
      return false;
    }

    user.permissions.contains(&perm.0)
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
      })
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

    group.permissions.contains(&perm.0)
      || inner
        .overlay_gid_grants
        .get(&gid)
        .map(|x| x.contains(&perm.0))
        .unwrap_or(false)
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
}
