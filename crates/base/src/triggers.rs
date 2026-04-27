use crate::flow::{
  FlowInstance, FlowItem, FlowMatchOperation, FlowPayload, FlowPayloadType, FlowType, StateMachine,
  Trigger,
};
use rind_core::prelude::*;

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
        if let Some(target_op) = target {
          if !match_operation(target_op, &trigger.payload) {
            return false;
          }
        }
        if let Some(branch_op) = branch {
          if !match_operation(branch_op, &trigger.payload) {
            return false;
          }
        }
        true
      } else if let Some(sig_name) = signal {
        if trigger.r#type != FlowType::Signal || *sig_name != trigger.name {
          return false;
        }
        if let Some(target_op) = target {
          if !match_operation(target_op, &trigger.payload) {
            return false;
          }
        }
        if let Some(branch_op) = branch {
          if !match_operation(branch_op, &trigger.payload) {
            return false;
          }
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
    FlowMatchOperation::Eq(v) => payload.to_string_payload() == v.as_str(),
    FlowMatchOperation::Options {
      binary,
      contains,
      r#as,
    } => {
      if let Some(true) = binary {
        matches!(payload, FlowPayload::Bytes(_))
      } else if let Some(needle) = contains {
        payload.to_string_payload().contains(needle.as_str())
      } else if let Some(filter) = r#as {
        subset_match(filter, &payload.to_json())
      } else {
        false
      }
    }
  }
}

pub fn trigger_events(
  triggers: Vec<Trigger>,
  sm: Option<&StateMachine>,
  dispatch: &RuntimeDispatcher,
) {
  for trigger in triggers {
    let mut resolved_triggers = Vec::new();

    let resolve_path = |branch: &crate::flow::FlowInstance, path: &str| -> String {
      if let crate::flow::FlowPayload::Json(j) = &branch.payload {
        let mut cur = j.into_json();
        for key in path.split('/') {
          if let Some(val) = cur.get(key) {
            cur = val.clone();
          }
        }
        cur
          .as_str()
          .map(|st| st.to_string())
          .unwrap_or_else(|| cur.to_string())
      } else {
        branch.payload.to_string_payload()
      }
    };

    if let Some(sm) = sm {
      match &trigger.payload {
        Some(serde_json::Value::String(s)) => {
          if let Some((state_name, path)) = s.rsplit_once('@') {
            if let Some(branches) = sm.states.get(state_name) {
              for branch in branches {
                let mut resolved_trigger = trigger.clone();
                let resolved = resolve_path(branch, path);
                resolved_trigger.payload = Some(serde_json::Value::String(resolved));
                resolved_triggers.push(resolved_trigger);
              }
            }
          } else {
            resolved_triggers.push(trigger.clone());
          }
        }
        Some(serde_json::Value::Object(map)) => {
          let mut primary_state = None;
          for v in map.values() {
            if let Some(s) = v.as_str() {
              if let Some(spec) = s.strip_prefix("state:") {
                if let Some((state_name, _)) = spec.rsplit_once('@') {
                  primary_state = Some(state_name.to_string());
                  break;
                }
              }
            }
          }

          if let Some(state_name) = primary_state {
            if let Some(branches) = sm.states.get(state_name.as_str()) {
              for branch in branches {
                let mut resolved_trigger = trigger.clone();
                let mut new_map = map.clone();
                for (_k, v) in new_map.iter_mut() {
                  if let Some(s) = v.as_str() {
                    if let Some(spec) = s.strip_prefix("state:") {
                      if let Some((s_name, path)) = spec.split_once('/') {
                        let branch_to_use = if s_name == state_name {
                          Some(branch)
                        } else {
                          sm.states.get(s_name).and_then(|b| b.first())
                        };
                        if let Some(b) = branch_to_use {
                          *v = serde_json::Value::String(resolve_path(b, path));
                        }
                      }
                    }
                  }
                }
                resolved_trigger.payload = Some(serde_json::Value::Object(new_map));
                resolved_triggers.push(resolved_trigger);
              }
            }
          } else {
            resolved_triggers.push(trigger.clone());
          }
        }
        _ => {
          resolved_triggers.push(trigger.clone());
        }
      }
    }

    for resolved_trigger in resolved_triggers {
      if let Some(script) = &resolved_trigger.script {
        let _ = std::process::Command::new("sh")
          .arg("-c")
          .arg(script)
          .spawn();
      } else if let Some(exec) = &resolved_trigger.exec {
        let mut cmd = std::process::Command::new(exec);
        if let Some(args) = &resolved_trigger.args {
          cmd.args(args);
        }
        let _ = cmd.spawn();
      } else if let Some(state) = &resolved_trigger.state {
        let mut p = rpayload!({ "name": state.clone() });
        if let Some(payload) = &resolved_trigger.payload {
          p = p.insert("payload", payload.clone());
        }
        let _ = dispatch.dispatch("flow", "set_state", p);
      } else if let Some(signal) = &resolved_trigger.signal {
        let mut p = rpayload!({ "name": signal.clone() });
        if let Some(payload) = &resolved_trigger.payload {
          p = p.insert("payload", payload.clone());
        }
        let _ = dispatch.dispatch("flow", "emit_signal", p);
      } else if let Some(service) = &resolved_trigger.service {
        let _ = dispatch.dispatch(
          "services",
          if let Some(true) = resolved_trigger.stop {
            "stop"
          } else {
            "start"
          },
          rpayload!({ "name": service.clone() }),
        );
      } else if let Some(timer) = &resolved_trigger.timer {
        let _ = dispatch.dispatch(
          "timer",
          if let Some(true) = resolved_trigger.stop {
            "stop"
          } else {
            "start"
          },
          rpayload!({ "name": timer.clone() }),
        );
      } else if let Some(socket) = &resolved_trigger.socket {
        let _ = dispatch.dispatch(
          "sockets",
          if let Some(true) = resolved_trigger.stop {
            "stop"
          } else {
            "start"
          },
          rpayload!({ "name": socket.clone() }),
        );
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

pub fn json_branch_key(value: &serde_json::Value, keys: &[Ustr]) -> Option<Vec<String>> {
  let obj = value.as_object()?;
  let mut out = Vec::new();
  for k in keys {
    let v = obj.get(k.as_str())?;
    out.push(v.to_string());
  }
  Some(out)
}

pub fn merge_json(a: &mut serde_json::Value, b: &serde_json::Value) {
  if let (Some(a_obj), Some(b_obj)) = (a.as_object_mut(), b.as_object()) {
    let mut forced = "";

    for (k, v) in b_obj {
      if forced == k {
        continue;
      }

      let k = k
        .strip_prefix("&")
        .map(|x| {
          forced = x;
          x
        })
        .unwrap_or(k);

      a_obj.insert(k.to_owned(), v.clone());
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
    FlowPayload::String(i) => Some(FlowMatchOperation::Eq(Ustr::from(i.as_str()))),
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

pub fn map_json_payload(branch_specs: &[Ustr], source: &FlowPayload) -> Option<FlowPayload> {
  let FlowPayload::Json(source_json) = source else {
    return None;
  };
  let source_json = source_json.into_json();
  let source_obj = source_json.as_object()?;
  let mut mapped = serde_json::Map::new();

  for spec in branch_specs {
    let key = &**spec;
    let source_key = branch_source_key(key);
    let target_key = branch_target_key(key);
    let value = source_obj.get(source_key)?;
    mapped.insert(target_key.to_string(), value.clone());
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
}
