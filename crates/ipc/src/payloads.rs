#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ListPayload {
  pub name: String,
  pub unit_type: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ServicePayload {
  pub name: String,
  pub force: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Run0AuthPayload {
  pub password: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct LoginPayload {
  pub username: String,
  pub password: Option<String>,
  pub tty: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct LogoutPayload {
  pub username: String,
  pub tty: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct NetworkPayload {
  pub iface: String,
  pub method: String,
  pub address: Option<String>,
  pub gateway: Option<String>,
}
