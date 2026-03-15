use crate::flow::{
  FlowInstance, FlowItem, FlowMatchOperation, FlowPayload, FlowPayloadType, FlowType,
};

pub fn check_condition(cond: &FlowItem, trigger: &FlowInstance) -> bool {
  match cond {
    FlowItem::Simple(name) => *name == trigger.name,
    FlowItem::Detailed {
      state,
      signal,
      target,
      branch,
    } => {
      if let Some(state_name) = state {
        if trigger.r#type != FlowType::State || *state_name != trigger.name {
          return false;
        }
        if let Some(branch_op) = branch {
          return match_operation(branch_op, &trigger.payload);
        }
        true
      } else if let Some(sig_name) = signal {
        if trigger.r#type != FlowType::Signal || *sig_name != trigger.name {
          return false;
        }
        if let Some(target_op) = target {
          return match_operation(target_op, &trigger.payload);
        }
        true
      } else {
        false
      }
    }
  }
}

pub fn match_operation(matcher: &FlowMatchOperation, payload: &FlowPayload) -> bool {
  match matcher {
    FlowMatchOperation::Eq(v) => payload.to_string_payload() == *v,
    FlowMatchOperation::Options {
      binary,
      contains,
      r#as,
    } => {
      if let Some(true) = binary {
        matches!(payload, FlowPayload::Bytes(_))
      } else if let Some(needle) = contains {
        payload.to_string_payload().contains(needle)
      } else if let Some(filter) = r#as {
        subset_match(filter, &payload.to_json())
      } else {
        false
      }
    }
  }
}

pub fn subset_match(filter: &serde_json::Value, payload: &serde_json::Value) -> bool {
  match (filter, payload) {
    (serde_json::Value::Object(f_tab), serde_json::Value::Object(p_tab)) => {
      for (key, f_val) in f_tab {
        let Some(p_val) = p_tab.get(key) else {
          return false;
        };
        if !subset_match(f_val, p_val) {
          return false;
        }
      }
      true
    }
    (serde_json::Value::Array(f_arr), serde_json::Value::Array(p_arr)) => {
      for f_val in f_arr {
        if !p_arr.iter().any(|p_val| subset_match(f_val, p_val)) {
          return false;
        }
      }
      true
    }
    (f, p) => f == p,
  }
}

pub fn json_branch_key(value: &serde_json::Value, keys: &[String]) -> Option<Vec<String>> {
  let obj = value.as_object()?;
  let mut out = Vec::new();
  for k in keys {
    let v = obj.get(k)?;
    out.push(v.to_string());
  }
  Some(out)
}

pub fn merge_json(a: &mut serde_json::Value, b: &serde_json::Value) {
  if let (Some(a_obj), Some(b_obj)) = (a.as_object_mut(), b.as_object()) {
    for (k, v) in b_obj {
      a_obj.insert(k.clone(), v.clone());
    }
  }
}

pub fn payload_signature(payload: &Option<FlowPayload>) -> String {
  match payload {
    Some(FlowPayload::Json(v)) => format!("json:{}", v.0),
    Some(FlowPayload::String(v)) => format!("str:{v}"),
    Some(FlowPayload::Bytes(v)) => format!("bytes:{}", v.len()),
    Some(FlowPayload::None(_)) => "none".to_string(),
    None => "none".to_string(),
  }
}

pub fn payload_to_filter(payload: &FlowPayload) -> Option<FlowMatchOperation> {
  match payload {
    FlowPayload::Json(i) => Some(FlowMatchOperation::Options {
      binary: None,
      contains: None,
      r#as: Some(i.into_json()),
    }),
    FlowPayload::String(i) => Some(FlowMatchOperation::Eq(i.clone())),
    FlowPayload::Bytes(_) | FlowPayload::None(_) => None,
  }
}

pub fn default_payload_for_type(t: FlowPayloadType) -> FlowPayload {
  match t {
    FlowPayloadType::Json => FlowPayload::Json(serde_json::json!({}).to_string().into()),
    FlowPayloadType::String => FlowPayload::String(String::new()),
    FlowPayloadType::Bytes => FlowPayload::Bytes(Vec::new()),
    FlowPayloadType::None => FlowPayload::None(false),
  }
}

pub fn run_eval(cmd: &str, args: Option<Vec<String>>) -> String {
  let out = std::process::Command::new(cmd)
    .args(args.unwrap_or_default())
    .output();
  match out {
    Ok(o) => String::from_utf8(o.stdout).unwrap_or_default(),
    Err(_) => String::new(),
  }
}

pub fn branch_target_key(spec: &str) -> &str {
  spec
    .split_once(':')
    .map(|(target, _)| target.trim())
    .unwrap_or(spec)
}

pub fn branch_source_key(spec: &str) -> &str {
  spec
    .split_once(':')
    .map(|(_, source)| source.trim())
    .unwrap_or(spec)
}

pub fn map_json_payload(branch_specs: &[String], source: &FlowPayload) -> Option<FlowPayload> {
  let FlowPayload::Json(source_json) = source else {
    return None;
  };
  let source_json = source_json.into_json();
  let source_obj = source_json.as_object()?;
  let mut mapped = serde_json::Map::new();

  for spec in branch_specs {
    let source_key = branch_source_key(spec);
    let target_key = branch_target_key(spec);
    let value = source_obj.get(source_key)?.clone();
    mapped.insert(target_key.to_string(), value);
  }

  Some(FlowPayload::Json(
    serde_json::Value::Object(mapped).to_string().into(),
  ))
}

pub fn payload_compatible(reference: Option<&FlowPayload>, thing: &FlowPayload) -> bool {
  let Some(reference) = reference else {
    return true;
  };
  match (reference, thing) {
    (FlowPayload::Json(a), FlowPayload::Json(b)) => json_subset(&a.into_json(), &b.into_json()),
    (FlowPayload::String(a), FlowPayload::String(b)) => a == b,
    (FlowPayload::None(_), _) => true,
    _ => true,
  }
}

fn json_subset(reference: &serde_json::Value, thing: &serde_json::Value) -> bool {
  let Some(ref_obj) = reference.as_object() else {
    return true;
  };
  let Some(thing_obj) = thing.as_object() else {
    return false;
  };

  let mut shared = 0usize;
  for (key, value) in ref_obj {
    if let Some(candidate) = thing_obj.get(key) {
      shared += 1;
      if value != candidate {
        return false;
      }
    }
  }

  if shared > 0 {
    return true;
  }

  ref_obj
    .values()
    .all(|v| thing_obj.values().any(|cv| cv == v))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn subset_match_nested() {
    let filter = serde_json::json!({"a":{"x":1},"b":[{"id":2}]});
    let payload = serde_json::json!({"a":{"x":1,"y":2},"b":[{"id":2},{"id":3}]});
    assert!(subset_match(&filter, &payload));
  }

  #[test]
  fn match_operation_variants() {
    assert!(match_operation(
      &FlowMatchOperation::Eq("abc".to_string()),
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
        contains: Some("ell".to_string()),
        r#as: None,
      },
      &FlowPayload::String("hello".to_string()),
    ));
  }

  #[test]
  fn json_key_extract_and_merge() {
    let mut left = serde_json::json!({"id":1,"a":"old"});
    let right = serde_json::json!({"a":"new","b":true});
    let key = json_branch_key(&left, &["id".to_string()]);
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
    let specs = vec!["tty:seat".to_string()];
    let mapped = map_json_payload(&specs, &payload).expect("should map");
    let FlowPayload::Json(j) = mapped else {
      panic!("expected json");
    };
    let obj = j.into_json();
    assert_eq!(obj.get("tty"), Some(&serde_json::json!("tty1")));
    assert!(obj.get("seat").is_none());
  }
}
