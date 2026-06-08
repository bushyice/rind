use rind_core::prelude::{PermissionExpr, PermissionId, PermissionStore};
use rind_core::user::UserStore;
use std::sync::Arc;

fn temp_dir(tag: &str) -> std::path::PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!("rind-perm-{tag}-{}-{now}", std::process::id()))
}

fn write_perms(path: &std::path::Path) {
  let mut out = Vec::new();
  // user 1000 -> perm 2001
  out.push(0u8);
  out.extend_from_slice(&1000u32.to_le_bytes());
  out.extend_from_slice(&1u16.to_le_bytes());
  out.extend_from_slice(&2001u16.to_le_bytes());
  // group 2000 -> perm 2002
  out.push(1u8);
  out.extend_from_slice(&2000u32.to_le_bytes());
  out.extend_from_slice(&1u16.to_le_bytes());
  out.extend_from_slice(&2002u16.to_le_bytes());
  std::fs::write(path, out).expect("permission blob should be written");
}

fn test_store() -> PermissionStore {
  let root = temp_dir("store");
  let etc = root.join("etc");
  std::fs::create_dir_all(&etc).expect("etc should exist");
  std::fs::write(
    etc.join("passwd"),
    "makano:x:1000:2000:makano:/home/makano:/bin/sh\n",
  )
  .expect("passwd should be written");
  std::fs::write(etc.join("shadow"), "makano:$6$hash$hash:1:0:99999:7:::\n")
    .expect("shadow should be written");
  std::fs::write(etc.join("group"), "wheel:x:2000:makano\n").expect("group should be written");
  write_perms(&etc.join("rperms"));

  let users = Arc::new(UserStore::load_from_root(&root).expect("user store should load"));
  PermissionStore::new(users)
}

#[test]
fn user_check_supports_perm_group_any_and_exact() {
  let store = test_store();
  let uid = 1000u32;
  let p_user = PermissionId(2001);
  let p_group = PermissionId(2002);
  let p_missing = PermissionId(2999);

  assert!(store.user_check(uid, &PermissionExpr::Perm(p_user)));
  assert!(store.user_check(uid, &PermissionExpr::Perm(p_group)));
  assert!(!store.user_check(uid, &PermissionExpr::Perm(p_missing)));
  assert!(store.user_check(uid, &PermissionExpr::Group("wheel".into())));

  assert!(store.user_check(
    uid,
    &PermissionExpr::Any(vec![
      PermissionExpr::Perm(p_missing),
      PermissionExpr::Perm(p_user)
    ])
  ));
  assert!(!store.user_check(
    uid,
    &PermissionExpr::Exact(vec![
      PermissionExpr::Perm(p_user),
      PermissionExpr::Perm(p_missing)
    ])
  ));
  assert!(store.user_check(
    uid,
    &PermissionExpr::Exact(vec![
      PermissionExpr::Perm(p_user),
      PermissionExpr::Group("wheel".into())
    ])
  ));
}

#[test]
fn overlay_grants_and_revokes_take_effect() {
  let store = test_store();
  let uid = 1000u32;
  let missing = PermissionId(2888);
  let present = PermissionId(2001);

  assert!(!store.user_has(uid, missing));
  store.grant_user(uid, missing);
  assert!(store.user_has(uid, missing));
  store.ungrant_user(uid, missing);
  assert!(!store.user_has(uid, missing));

  assert!(store.user_has(uid, present));
  store.ungrant_user(uid, present);
  assert!(!store.user_has(uid, present));
  store.grant_user(uid, present);
  assert!(store.user_has(uid, present));
}
