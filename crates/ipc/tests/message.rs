use rind_ipc::ser::deser_from_vec;
use rind_ipc::{Message, MessageType};

#[test]
fn message_roundtrip_serialization_contract() {
  let msg = Message::from_action("service.start")
    .with_slice(b"{\"name\":\"units:demo\"}")
    .from_uid(1000)
    .from_gid(1000)
    .from_pid(4242);

  let raw = msg.clone().as_bytes();
  let decoded: Message = deser_from_vec(&raw, false).expect("message should deserialize");
  assert_eq!(decoded.action, msg.action);
  assert_eq!(decoded.payload, msg.payload);
  assert_eq!(decoded.from_uid, Some(1000));
  assert_eq!(decoded.from_gid, Some(1000));
  assert_eq!(decoded.from_pid, Some(4242));
}

#[test]
fn parse_payload_errors_when_missing_or_invalid() {
  let missing = Message::from_type(MessageType::Valid);
  assert!(missing.parse_payload::<u32>().is_err());
}

#[test]
fn with_vec_and_parse_vec_payload_roundtrip_property() {
  for len in 0..40usize {
    let values: Vec<u32> = (0..len as u32).map(|v| v * 3).collect();
    let msg = Message::from_action("list").with_vec(values.clone());
    let parsed = msg
      .parse_vec_payload::<u32>()
      .expect("payload should deserialize");
    assert_eq!(parsed, values);
  }
}
