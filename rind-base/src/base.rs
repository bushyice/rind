use rind_core::metadata::Metadata;

pub mod services;

pub fn initiate() {
  let _metadata: Metadata = services::services_metadata("units");
}
