use rind_flow::triggers::{
  subset_match, match_operation, json_branch_key, merge_json, branch_target_key, branch_source_key,
  map_json_payload,
};
use rind_flow::{FlowMatchOperation, FlowPayload};
use rind_core::prelude::Ustr;

#[test]
fn subset_match_nested() {
  let filter = serde_json::json!({"a":{"x":1},"b":[{"id":2}]});
  let payload = serde_json::json!({"a":{"x":1,"y":2},"b":[{"id":2},{"id":3}]});
  assert!(subset_match(&filter, &payload));
}

#[test]
fn match_operation_variants() {
  assert!(match_operation(
    &FlowMatchOperation::Eq(Ustr::from("abc")),
    &FlowPayload::String("abc".to_string()),
  ));
  assert!(match_operation(
    &FlowMatchOperation::Options {
      binary: Some(true),
      contains: None,
      r#as: None,
    },
    &FlowPayload::Bytes(vec![1, 2]),
  ));
  assert!(match_operation(
    &FlowMatchOperation::Options {
      binary: None,
      contains: Some(Ustr::from("ell")),
      r#as: None,
    },
    &FlowPayload::String("hello".to_string()),
  ));
}

#[test]
fn json_key_extract_and_merge() {
  let mut left = serde_json::json!({"id":1,"a":"old"});
  let right = serde_json::json!({"a":"new","b":true});
  let key = json_branch_key(&left, &[Ustr::from("id")]);
  assert_eq!(key, Some(vec!["1".to_string()]));
  merge_json(&mut left, &right);
  assert_eq!(left["a"], serde_json::json!("new"));
  assert_eq!(left["b"], serde_json::json!(true));
}

#[test]
fn branch_key_parsing() {
  assert_eq!(branch_target_key("tty:seat"), "tty");
  assert_eq!(branch_source_key("tty:seat"), "seat");
  assert_eq!(branch_target_key("id"), "id");
  assert_eq!(branch_source_key("id"), "id");
}

#[test]
fn map_json_payload_renames_keys() {
  let payload = FlowPayload::Json(
    serde_json::json!({"id": "u1", "seat": "tty1", "user": "makano"})
      .to_string()
      .into(),
  );
  let specs = vec![Ustr::from("tty:seat")];
  let mapped = map_json_payload(&specs, &payload).expect("should map");
  let FlowPayload::Json(j) = mapped else {
    panic!("expected json");
  };
  let obj = j.into_json();
  assert_eq!(obj.get("tty"), Some(&serde_json::json!("tty1")));
  assert!(obj.get("seat").is_none());
}
