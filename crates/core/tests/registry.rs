use rind_core::prelude::rslvns;
use rind_core::prelude::{Metadata, MetadataRegistry, Model, NamedItem, Ustr};
use rind_macros::model;

#[model(meta_name = name, meta_fields(name, run))]
struct Service {
  name: Ustr,
  run: Option<Ustr>,
}

#[model(meta_name = target, meta_fields(target))]
struct Mount {
  target: Ustr,
}

#[test]
fn lookup_by_group_and_item_name() {
  let mut metadata = Metadata::new("units")
    .of::<Service>("service")
    .of::<Mount>("mount");

  let mut registry = MetadataRegistry::default();

  let src = r#"
[[service]]
name = "web"
run = "/bin/webd"

[[service]]
name = "api"
"#;

  registry
    .load_group_from_toml(&mut metadata, "demo", src)
    .expect("group should parse");
  registry.insert_metadata(metadata);
  registry
    .ensure_index_for_type::<Service>("units")
    .expect("index build should succeed");

  let web = registry
    .find::<Service>("units", rslvns!("demo", "web"))
    .expect("service should be indexed by unit:item");
  assert_eq!(web.name.as_str(), "web");
  assert_eq!(web.run.as_ref().map(|x| x.as_str()), Some("/bin/webd"));

  assert!(
    registry
      .find::<Service>("units", rslvns!("demo", "missing"))
      .is_none()
  );
  assert!(
    registry
      .find::<Service>("units", rslvns!("missing", "web"))
      .is_none()
  );
}
