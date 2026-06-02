use rind_core::types::Ustr;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ListPayload {
  pub name: Ustr,
  pub unit_type: String,
  pub scope: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct SSPayload {
  pub name: String,
  #[serde(default)]
  pub force: bool,
  #[serde(default)]
  pub persist: bool,
  #[serde(default)]
  pub unit_type: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Run0AuthPayload {
  pub password: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct LoginPayload {
  pub username: String,
  pub password: Option<String>,
  pub seat: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct LogoutPayload {
  pub username: String,
  pub session_id: u64,
  pub seat: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct NetworkPayload {
  pub iface: String,
  pub method: String,
  pub address: Option<String>,
  pub gateway: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ScopeCreatePayload {
  pub scope: String,
  #[serde(default)]
  pub lifetime_state: Option<String>,
  #[serde(default)]
  pub attributes: std::collections::HashMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ScopeDestroyPayload {
  pub scope: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct PermissionPayload {
  pub subject: String,
  pub permission: String,
  #[serde(default)]
  pub group: bool,
}
