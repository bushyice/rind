use rind_core::prelude::{Model, NamedItem, PermissionId, model};

pub static PERM_SYSTEM_SERVICES: PermissionId = PermissionId(1000);
pub static PERM_LOGIN: PermissionId = PermissionId(1001);

#[model(
  meta_name = name,
  meta_fields(
    name, id
  )
)]
pub struct Permission {
  pub name: String,
  pub id: u16,
}
