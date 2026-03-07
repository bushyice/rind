use serde::{Deserialize, Serialize};

use crate::services::{Service, StopMode, reconcile_state_branching, start_service, stop_service};
use rind_common::error::{report_error, rw_write};

use super::*;

#[derive(Debug, Clone, Copy)]
pub enum FlowChangeAction {
  Apply,
  Revert,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Trigger {
  script: Option<String>,
  exec: Option<String>,
  args: Option<Vec<String>>,
  state: Option<String>,
  signal: Option<String>,
  payload: Option<serde_json::Value>,
}

impl crate::store::Store {
  pub fn check_flow(
    &self,
    r#type: FlowType,
    name: &String,
    payload: &Option<FlowPayload>,
  ) -> Option<&FlowDefinitionBase> {
    let flowdef = if matches!(r#type, FlowType::State) {
      &self.lookup::<StateDefinition>(&name)?.0
    } else {
      &self.lookup::<SignalDefinition>(&name)?.0
    };
    let Some(p) = payload else {
      return Some(flowdef);
    };
    if match p {
      FlowPayload::Bytes(_) => matches!(flowdef.payload, FlowPayloadType::Bytes),
      FlowPayload::String(_) => matches!(flowdef.payload, FlowPayloadType::String),
      FlowPayload::Json(_) => matches!(flowdef.payload, FlowPayloadType::Json),
      FlowPayload::None(_) => matches!(flowdef.payload, FlowPayloadType::None),
    } {
      Some(flowdef)
    } else {
      None
    }
  }

  pub fn set_state(
    &mut self,
    name: String,
    payload: Option<FlowPayload>,
    except: Option<&Vec<String>>,
  ) -> anyhow::Result<()> {
    // special keys
    let branches = match self.check_flow(FlowType::State, &name, &payload) {
      None => return Err(anyhow::anyhow!("State trigger validation failed.")),
      Some(e) => e.branch.clone(),
    };

    let instance = FlowInstance {
      name: name.clone(),
      payload: if let Some(p) = payload {
        p
      } else {
        FlowPayload::None(false)
      },
      r#type: FlowType::State,
    };

    self.check_triggers(&instance, FlowChangeAction::Apply);
    self
      .broadcast(&instance, FlowChangeAction::Apply, except)
      .ok();

    let entry = self.states.entry(name.clone()).or_insert_with(Vec::new);

    match &instance.payload {
      FlowPayload::String(_) | FlowPayload::Bytes(_) | FlowPayload::None(_) => {
        entry.clear();
        entry.push(instance.clone());
      }

      FlowPayload::Json(new_json) => {
        let branch_keys = if let Some(b) = &branches {
          b
        } else {
          &vec!["id".to_string()]
        };

        let new_key = json_branch_key(&new_json.into_json(), branch_keys)
          .ok_or_else(|| anyhow::anyhow!("Invalid JSON branch keys"))?;

        let mut found = false;

        for branch in entry.iter_mut() {
          if let FlowPayload::Json(json) = &mut branch.payload {
            let mut existing_json = json.into_json();
            let existing_key = json_branch_key(&existing_json, branch_keys);

            // println!("{existing_key:?}::{new_key:?}");

            if existing_key == Some(new_key.clone()) {
              merge_json(&mut existing_json, &new_json.into_json());
              json.swap(existing_json);
              found = true;
              break;
            }
          }
        }

        if !found {
          entry.push(instance.clone());
        }
      }
    }

    reconcile_state_branching(self, &instance, FlowChangeAction::Apply);
    self.save_state();
    Ok(())
  }

  pub fn remove_state(
    &mut self,
    name: &str,
    filter: Option<FlowMatchOperation>,
    except: Option<&Vec<String>>,
  ) {
    if let Some(branches) = self.states.remove(name) {
      let (to_keep, mut to_remove): (Vec<_>, Vec<_>) = branches.into_iter().partition(|branch| {
        if let Some(filter) = &filter {
          !match_operation(filter, &branch.payload)
        } else {
          true
        }
      });

      if let Some(_) = &filter {
        for branch in &mut to_remove {
          branch.r#type = FlowType::State;
          self.check_triggers(branch, FlowChangeAction::Revert);
          self
            .broadcast(branch, FlowChangeAction::Revert, except)
            .ok();
          reconcile_state_branching(self, branch, FlowChangeAction::Revert);
        }
      }

      if !to_keep.is_empty() {
        self.states.insert(name.to_string(), to_keep);
      }

      self.save_state();
    }
  }

  pub fn emit_signal(
    &mut self,
    name: String,
    payload: Option<FlowPayload>,
    except: Option<&Vec<String>>,
  ) -> anyhow::Result<()> {
    if let Some(_) = self.check_flow(FlowType::Signal, &name, &payload) {
      let instance = FlowInstance {
        name: name.clone(),
        payload: if let Some(p) = payload {
          p
        } else {
          FlowPayload::None(false)
        },
        r#type: FlowType::State,
      };

      self.check_triggers(&instance, FlowChangeAction::Apply);
      self
        .broadcast(&instance, FlowChangeAction::Apply, except)
        .ok();

      Ok(())
    } else {
      Err(anyhow::anyhow!("Signal trigger validation failed."))
    }
  }

  pub fn broadcast(
    &mut self,
    instance: &FlowInstance,
    action: FlowChangeAction,
    except: Option<&Vec<String>>,
  ) -> anyhow::Result<()> {
    println!("{action:?} {except:?}");

    // Stage 1: Collection
    let subs = match self.check_flow(
      instance.r#type.clone(),
      &instance.name,
      &Some(instance.payload.clone()),
    ) {
      None => return Err(anyhow::anyhow!("State trigger validation failed.")),
      Some(e) => e.subscribers.clone(),
    };

    let services = self
      .items_mut::<Service>()
      .filter(|(_, s)| s.transport.is_some());
    let mut transports = rw_write(&TRANSPORTS, "transports write in broadcast");

    // Stage 2: Context Buildup
    let empty = &Vec::new();
    let mut ctx = TransportContext::new(
      &instance.payload,
      except.unwrap_or(empty),
      if matches!(action, FlowChangeAction::Revert) {
        TransportMessageAction::Remove
      } else {
        TransportMessageAction::Set
      },
    );

    // Stage 3: Actual Trigger
    if let Some(subs) = subs {
      for sub in subs {
        if let Some(transport) = transports.get_mut(sub.as_id()) {
          transport.recv(&mut ctx, &instance, None)?;
        } else {
          report_error("broadcast missing transport", sub.as_id().0.clone());
        }
      }
    }

    for (_, mut serv) in services {
      let Some(transport_method) = serv.transport.as_ref() else {
        continue;
      };
      if let Some(transport) = transports.get_mut(transport_method.as_id()) {
        transport.recv(&mut ctx, &instance, Some(&mut serv))?;
      } else {
        report_error(
          "broadcast missing service transport",
          transport_method.as_id().0.clone(),
        );
      }
    }

    Ok(())
  }

  pub fn check_triggers(&mut self, trigger: &FlowInstance, action: FlowChangeAction) {
    let mut to_start_services = Vec::new();
    let mut to_stop_services = Vec::new();

    let state_def = if matches!(trigger.r#type, FlowType::State) {
      if let Some(def) = self.lookup::<StateDefinition>(&trigger.name) {
        &def.0
      } else {
        return;
      }
    } else {
      if let Some(def) = self.lookup::<SignalDefinition>(&trigger.name) {
        &def.0
      } else {
        return;
      }
    };

    for (unit_name, service) in self.items::<Service>() {
      let comp_id = format!("{}@{}", unit_name.to_string(), service.name);
      if let Some(targets) = &state_def.broadcast {
        if !targets.contains(&comp_id) {
          continue;
        }
      }

      if let Some(start_on) = &service.start_on {
        if start_on.iter().any(|c| check_condition(c, &trigger)) {
          if matches!(action, FlowChangeAction::Revert) {
            to_stop_services.push(comp_id.clone());
          } else {
            to_start_services.push(comp_id.clone());
          }
        }
      }

      if let Some(stop_on) = &service.stop_on {
        if stop_on.iter().any(|c| check_condition(c, &trigger)) {
          if matches!(action, FlowChangeAction::Revert) {
            to_start_services.push(comp_id.clone());
          } else {
            to_stop_services.push(comp_id.clone());
          }
        }
      }
    }

    for name in to_stop_services {
      if let Some(service) = self.lookup_mut::<Service>(&name) {
        stop_service(service, StopMode::Graceful);
      }
    }
    for name in to_start_services {
      if let Some(service) = self.lookup_mut::<Service>(&name) {
        // Store payload state
        // if let Some(p) = &payload {
        //   service.active_payload = Some(serde_json::to_string(p).unwrap_or_default());
        // }
        start_service(service);
      }
    }
  }
}

pub fn subset_match(filter: &serde_json::Value, payload: &serde_json::Value) -> bool {
  match (filter, payload) {
    (serde_json::Value::Object(f_tab), serde_json::Value::Object(p_tab)) => {
      for (key, f_val) in f_tab.iter() {
        if let Some(p_val) = p_tab.get(key) {
          if !subset_match(f_val, p_val) {
            return false;
          }
        } else {
          return false;
        }
      }
      true
    }
    (serde_json::Value::Array(f_arr), serde_json::Value::Array(p_arr)) => {
      for f_val in f_arr {
        if !p_arr.iter().any(|p_val| subset_match(f_val, p_val)) {
          return false; // Item missing
        }
      }
      true
    }
    (f, p) => f == p,
  }
}

fn check_condition(cond: &FlowItem, trigger: &FlowInstance) -> bool {
  let cond_state = match cond {
    FlowItem::Detailed { state: Some(s), .. } => Some(s.clone()),
    FlowItem::Simple(s) => Some(s.clone()),
    _ => None,
  };
  let cond_sig = match cond {
    FlowItem::Detailed {
      signal: Some(s), ..
    } => Some(s.clone()),
    _ => None,
  };

  match cond {
    FlowItem::Simple(_) => {
      if let Some(s) = cond_state {
        if s == *trigger.name {
          return true;
        }
      }
      if let Some(s) = cond_sig {
        if s == *trigger.name {
          return true;
        }
      }
      false
    }
    FlowItem::Detailed {
      state,
      signal,
      target,
      branch,
    } => {
      if let Some(state) = state {
        if matches!(trigger.r#type, FlowType::State) {
          if let Some(branch) = branch {
            match_operation(branch, &trigger.payload)
          } else {
            if *state == *trigger.name { true } else { false }
          }
        } else {
          false
        }
      } else if let Some(sig) = signal {
        if matches!(trigger.r#type, FlowType::Signal) {
          if let Some(target) = target {
            match_operation(target, &trigger.payload)
          } else {
            if *sig == *trigger.name { true } else { false }
          }
        } else {
          false
        }
      } else {
        false
      }
    }
  }
}

fn match_operation(matcher: &FlowMatchOperation, payload: &FlowPayload) -> bool {
  match matcher {
    FlowMatchOperation::Eq(s) => payload.to_string() == *s,
    FlowMatchOperation::Options {
      binary,
      contains,
      r#as,
    } => {
      if let Some(true) = binary {
        matches!(payload, FlowPayload::Bytes(_))
      } else if let Some(contains) = contains {
        payload.contains(contains)
      } else if let Some(filter) = r#as {
        match payload {
          FlowPayload::Json(payload) => subset_match(filter, &payload.into_json()),
          _ => false,
        }
      } else {
        false
      }
    }
  }
}

fn json_branch_key(value: &serde_json::Value, keys: &[String]) -> Option<Vec<String>> {
  let obj = value.as_object()?;

  let mut out = Vec::new();

  for k in keys {
    let v = obj.get(k)?;
    out.push(v.to_string());
  }

  Some(out)
}

fn merge_json(a: &mut serde_json::Value, b: &serde_json::Value) {
  if let (Some(a_obj), Some(b_obj)) = (a.as_object_mut(), b.as_object()) {
    for (k, v) in b_obj {
      a_obj.insert(k.clone(), v.clone());
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn subset_match_for_nested_payloads() {
    let filter = serde_json::json!({"a":{"x":1},"b":[{"id":2}]});
    let payload = serde_json::json!({"a":{"x":1,"y":2},"b":[{"id":2},{"id":3}]});
    assert!(subset_match(&filter, &payload));
  }

  #[test]
  fn match_operation_variants() {
    assert!(match_operation(
      &FlowMatchOperation::Eq("abc".to_string()),
      &FlowPayload::String("abc".to_string())
    ));
    assert!(match_operation(
      &FlowMatchOperation::Options {
        binary: Some(true),
        contains: None,
        r#as: None
      },
      &FlowPayload::Bytes(vec![1, 2])
    ));
    assert!(match_operation(
      &FlowMatchOperation::Options {
        binary: None,
        contains: Some("ell".to_string()),
        r#as: None
      },
      &FlowPayload::String("hello".to_string())
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
}
