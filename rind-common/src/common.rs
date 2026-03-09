pub mod config;
pub mod error;
pub mod fs_async;
pub mod logger;
pub mod utils;

#[derive(Debug, Copy, Clone, serde::Deserialize, serde::Serialize)]
pub enum UnitType {
  Flow,
  State,
  Signal,
  Service,
  Mount,
  Unit,
  Unknown,
}
