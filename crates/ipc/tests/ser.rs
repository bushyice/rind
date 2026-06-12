use rind_ipc::ser::{
  SerializeSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many,
};

#[test]
fn unit_serialized_roundtrip() {
  let item = UnitSerialized {
    name: "u".to_string().into(),
    services: 2,
    active_services: 1,
    mounts: 1,
    mounted: 1,
    ..Default::default()
  };
  let encoded = item.serialize();
  let decoded = UnitSerialized::from_bytes(&encoded);
  assert_eq!(decoded.name, "u".to_string().into());
  assert_eq!(decoded.services, 2);
}

#[test]
fn invalid_input_falls_back() {
  let decoded = UnitSerialized::from_bytes(b"bad-json");
  assert_eq!(decoded.name, "".to_string().into());
  assert_eq!(UnitSerialized::many_from_bytes(b"bad-json").len(), 0);
}

#[test]
fn serialize_many_and_nested_types() {
  let services = vec![ServiceSerialized {
    name: "svc".to_string().into(),
    last_state: "Active".to_string(),
    after: Some(vec!["db".to_string().into()]),
    restart: true,
    run: vec!["hello".to_string().into()],
    pid: Some(1),
    description: None,
  }];
  let out = serialize_many(&services);
  assert!(!out.is_empty());

  let unit_items = UnitItemsSerialized {
    mounts: vec![],
    services,
    ..Default::default()
  };
  assert!(!unit_items.serialize().is_empty());
}
