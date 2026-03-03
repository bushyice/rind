pub mod config;
pub mod logger;
pub mod utils;

#[derive(Debug, Copy, Clone, serde::Deserialize, serde::Serialize)]
pub enum UnitType {
  Socket,
  Service,
  Mount,
  Unit,
  Unknown,
}
