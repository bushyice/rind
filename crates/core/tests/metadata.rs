use rind_core::prelude::{Metadata, Model, NamedItem, Ustr};
use rind_macros::model;
use toml::Value;

#[model(meta_name = name, meta_fields(name, run))]
struct Service {
  name: Ustr,
}

#[test]
fn parse_grouped_metadata_from_toml() {
  let mut metadata = Metadata::new("unit").of::<Service>("service");

  let src = r#"
[[service]]
name = "web"
run = "/bin/webd"

[[service]]
name = "api"
"#;

  metadata
    .from_toml(src, "demo")
    .expect("toml should parse into group");

  let services = metadata
    .get_in_group::<Service>("demo")
    .expect("service vec should exist in group");
  assert_eq!(services.len(), 2);
  assert_eq!(services[0].name.as_str(), "web");
  assert_eq!(services[1].name.as_str(), "api");
}

#[test]
fn insert_value_type_mismatch_errors() {
  let mut metadata = Metadata::new("unit").of::<Service>("service");

  let err = metadata
    .insert_value("service", Value::String("not-an-array".to_string()), "demo")
    .expect_err("non-array value should fail Vec<Service> parser");

  assert!(!err.to_string().is_empty());
}

#[test]
fn unknown_key_is_ignored_by_collect() {
  let mut metadata = Metadata::new("unit").of::<Service>("service");

  let src = r#"
[[mount]]
name = "data"
"#;

  metadata
    .from_toml(src, "demo")
    .expect("unknown top-level arrays are ignored");
  assert!(metadata.get_in_group::<Service>("demo").is_none());
}
