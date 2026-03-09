use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Command;

use crate::services::{
  Service, StopMode, prepare_service_transport_from_states, reconcile_state_branching,
  start_service, stop_service,
};
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
    let mut guard = HashSet::new();
    self.set_state_internal(name, payload, except, &mut guard)
  }

  fn set_state_internal(
    &mut self,
    name: String,
    payload: Option<FlowPayload>,
    except: Option<&Vec<String>>,
    guard: &mut HashSet<String>,
  ) -> anyhow::Result<()> {
    let branch_sig = payload_signature(&payload);
    let guard_key = format!("apply::{name}::{branch_sig}");
    if guard.contains(&guard_key) {
      return Ok(());
    }
    guard.insert(guard_key.clone());

    // special keys
    let branches = match self.check_flow(FlowType::State, &name, &payload) {
      None => {
        guard.remove(&guard_key);
        return Err(anyhow::anyhow!("State trigger validation failed."));
      }
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
        let branch_keys = branches
          .as_ref()
          .map(|b| {
            b.iter()
              .map(|key| branch_target_key(key.as_str()).to_string())
              .collect::<Vec<String>>()
          })
          .unwrap_or_else(|| vec!["id".to_string()]);

        let new_key = json_branch_key(&new_json.into_json(), &branch_keys).ok_or_else(|| {
          guard.remove(&guard_key);
          anyhow::anyhow!("Invalid JSON branch keys")
        })?;

        let mut found = false;

        for branch in entry.iter_mut() {
          if let FlowPayload::Json(json) = &mut branch.payload {
            let mut existing_json = json.into_json();
            let existing_key = json_branch_key(&existing_json, &branch_keys);

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
    self.reconcile_state_transcendence(&instance, FlowChangeAction::Apply, except, guard);
    self.reconcile_activate_on_none(except, guard);
    self.save_state();
    guard.remove(&guard_key);
    Ok(())
  }

  pub fn remove_state(
    &mut self,
    name: &str,
    filter: Option<FlowMatchOperation>,
    except: Option<&Vec<String>>,
  ) {
    let mut guard = HashSet::new();
    self.remove_state_internal(name, filter, except, &mut guard);
  }

  fn remove_state_internal(
    &mut self,
    name: &str,
    filter: Option<FlowMatchOperation>,
    except: Option<&Vec<String>>,
    guard: &mut HashSet<String>,
  ) {
    if let Some(branches) = self.states.remove(name) {
      let (to_keep, mut to_remove): (Vec<_>, Vec<_>) = if let Some(filter) = &filter {
        branches
          .into_iter()
          .partition(|branch| !match_operation(filter, &branch.payload))
      } else {
        (Vec::new(), branches)
      };

      for branch in &mut to_remove {
        branch.r#type = FlowType::State;
        let guard_key = format!(
          "revert::{}::{}",
          branch.name,
          payload_signature(&Some(branch.payload.clone()))
        );
        if guard.contains(&guard_key) {
          continue;
        }
        guard.insert(guard_key.clone());

        self.check_triggers(branch, FlowChangeAction::Revert);
        self
          .broadcast(branch, FlowChangeAction::Revert, except)
          .ok();
        reconcile_state_branching(self, branch, FlowChangeAction::Revert);
        self.reconcile_state_transcendence(branch, FlowChangeAction::Revert, except, guard);
        self.reconcile_activate_on_none(except, guard);
        guard.remove(&guard_key);
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
        r#type: FlowType::Signal,
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

  fn reconcile_state_transcendence(
    &mut self,
    source: &FlowInstance,
    action: FlowChangeAction,
    except: Option<&Vec<String>>,
    guard: &mut HashSet<String>,
  ) {
    let dependents = self
      .items::<StateDefinition>()
      .filter_map(|(unit_name, def)| {
        let after = def.after.as_ref()?;
        if !after.iter().any(|cond| check_condition(cond, source)) {
          return None;
        }
        let source_payload = if let Some(_) = def.auto_payload {
          &auto_payload_for(&def, Some(&source.payload))
        } else {
          &source.payload
        };
        let payload = transcendent_payload_for(def, source_payload)?;
        let all_active = after
          .iter()
          .all(|cond| self.condition_is_active(cond, Some(&payload)));
        if matches!(action, FlowChangeAction::Apply) {
          if !all_active {
            return None;
          }
        } else if all_active {
          return None;
        }
        Some((format!("{}@{}", unit_name.to_string(), def.name), payload))
      })
      .filter(|(name, _)| name != &source.name)
      .collect::<Vec<_>>();

    for (dependent, payload) in dependents {
      if matches!(action, FlowChangeAction::Apply) {
        if let Err(err) = self.set_state_internal(dependent.clone(), Some(payload), except, guard) {
          report_error("state transcendence apply failed", err);
        }
      } else {
        self.remove_state_internal(
          dependent.as_str(),
          payload_to_filter(&payload),
          except,
          guard,
        );
      }
    }
  }

  pub fn reconcile_activate_on_none_boot(&mut self) {
    let mut guard = HashSet::new();
    self.reconcile_activate_on_none(None, &mut guard);
  }

  fn reconcile_activate_on_none(
    &mut self,
    except: Option<&Vec<String>>,
    guard: &mut HashSet<String>,
  ) {
    let targets = self
      .items::<StateDefinition>()
      .filter_map(|(unit_name, def)| {
        let deps = def.activate_on_none.as_ref()?;
        let should_activate = deps
          .iter()
          .all(|name| self.states.get(name).map(|v| v.is_empty()).unwrap_or(true));
        let full_name = format!("{}@{}", unit_name.to_string(), def.name);
        Some((full_name, should_activate, auto_payload_for(def, None)))
      })
      .collect::<Vec<(String, bool, FlowPayload)>>();

    for (name, should_activate, payload) in targets {
      let currently_active = self
        .states
        .get(&name)
        .map(|v| !v.is_empty())
        .unwrap_or(false);

      if should_activate && !currently_active {
        if let Err(err) = self.set_state_internal(name, Some(payload), except, guard) {
          report_error("activate-on-none set failed", err);
        }
      } else if !should_activate && currently_active {
        self.remove_state_internal(name.as_str(), None, except, guard);
      }
    }
  }

  fn condition_is_active(&self, cond: &FlowItem, payload: Option<&FlowPayload>) -> bool {
    for branches in self.states.values() {
      for branch in branches {
        let mut state = branch.clone();
        state.r#type = FlowType::State;
        if check_condition(cond, &state) && payload_compatible(payload, &state.payload) {
          return true;
        }
      }
    }
    false
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
      if transport_method.as_id().0.trim().is_empty() {
        continue;
      }
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
    let states_snapshot = self.states.clone();

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
        prepare_service_transport_from_states(service, &states_snapshot, Some(trigger));
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
            *state == *trigger.name && match_operation(branch, &trigger.payload)
          } else {
            if *state == *trigger.name { true } else { false }
          }
        } else {
          false
        }
      } else if let Some(sig) = signal {
        if matches!(trigger.r#type, FlowType::Signal) {
          if let Some(target) = target {
            *sig == *trigger.name && match_operation(target, &trigger.payload)
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

fn payload_signature(payload: &Option<FlowPayload>) -> String {
  match payload {
    // TODO: make it so only special keys from the definition are signatured for json
    Some(FlowPayload::Json(v)) => format!("json:{}", v.to_string()),
    Some(FlowPayload::String(v)) => format!("str:{v}"),
    Some(FlowPayload::Bytes(v)) => format!("bytes:{}", v.len()),
    Some(FlowPayload::None(_)) => "none".to_string(),
    None => "none".to_string(),
  }
}

fn payload_to_filter(payload: &FlowPayload) -> Option<FlowMatchOperation> {
  match payload {
    FlowPayload::Json(i) => Some(FlowMatchOperation::Options {
      binary: None,
      contains: None,
      r#as: Some(i.into_json()),
    }),
    FlowPayload::Bytes(_) => None,
    FlowPayload::String(i) => Some(FlowMatchOperation::Eq(i.clone())),
    FlowPayload::None(_) => None,
  }
}

fn auto_payload_for(def: &StateDefinition, _payload: Option<&FlowPayload>) -> FlowPayload {
  let Some(cfg) = &def.auto_payload else {
    return default_payload_for_type(def.payload);
  };

  let output = if let Some(eval) = &cfg.eval {
    run_eval(eval.as_str(), cfg.args.clone())
  } else {
    String::new()
  };
  let lines = output
    .lines()
    .map(|x| x.trim().to_string())
    .filter(|x| !x.is_empty())
    .collect::<Vec<String>>();

  match def.payload {
    FlowPayloadType::Json => {
      let mut obj = serde_json::Map::new();
      match &cfg.insert {
        Some(AutoPayloadInsert::One(key)) if key == "root" => {
          if lines.len() == 1 {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&lines[0]) {
              return FlowPayload::Json(v.to_string().into());
            }
            return FlowPayload::Json(
              serde_json::Value::String(lines[0].clone())
                .to_string()
                .into(),
            );
          }
          return FlowPayload::Json(
            serde_json::to_value(lines)
              .unwrap_or_default()
              .to_string()
              .into(),
          );
        }
        Some(AutoPayloadInsert::One(key)) => {
          if let Some(first) = lines.first() {
            obj.insert(key.clone(), serde_json::Value::String(first.clone()));
          }
        }
        Some(AutoPayloadInsert::Many(keys)) => {
          for (i, key) in keys.iter().enumerate() {
            if let Some(line) = lines.get(i) {
              obj.insert(key.clone(), serde_json::Value::String(line.clone()));
            }
          }
        }
        None => {
          if let Some(first) = lines.first() {
            obj.insert(
              "value".to_string(),
              serde_json::Value::String(first.clone()),
            );
          }
        }
      }
      FlowPayload::Json(serde_json::Value::Object(obj).to_string().into())
    }
    FlowPayloadType::String => FlowPayload::String(lines.join("\n")),
    FlowPayloadType::Bytes => FlowPayload::Bytes(output.into_bytes()),
    FlowPayloadType::None => FlowPayload::None(false),
  }
}

fn run_eval(cmd: &str, args: Option<Vec<String>>) -> String {
  let out = Command::new(cmd).args(args.unwrap_or(Vec::new())).output();
  match out {
    Ok(o) => String::from_utf8(o.stdout).unwrap_or_default(),
    Err(err) => {
      report_error("auto-payload eval failed", err);
      String::new()
    }
  }
}

fn default_payload_for_type(t: FlowPayloadType) -> FlowPayload {
  match t {
    FlowPayloadType::Json => FlowPayload::Json(serde_json::json!({}).to_string().into()),
    FlowPayloadType::String => FlowPayload::String(String::new()),
    FlowPayloadType::Bytes => FlowPayload::Bytes(Vec::new()),
    FlowPayloadType::None => FlowPayload::None(false),
  }
}

fn transcendent_payload_for(
  def: &StateDefinition,
  source_payload: &FlowPayload,
) -> Option<FlowPayload> {
  match def.payload {
    FlowPayloadType::Json => {
      if let Some(branch_specs) = &def.branch {
        map_json_payload(branch_specs, source_payload)
      } else if let FlowPayload::Json(_) = source_payload {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::String => {
      if let FlowPayload::String(_) = source_payload {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::Bytes => {
      if let FlowPayload::Bytes(_) = source_payload {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::None => Some(FlowPayload::None(false)),
  }
}

fn payload_compatible(reference: Option<&FlowPayload>, thing: &FlowPayload) -> bool {
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
    if let Some(candidate_value) = thing_obj.get(key) {
      shared += 1;
      if value != candidate_value {
        return false;
      }
    }
  }

  if shared > 0 {
    return true;
  }

  ref_obj.values().all(|value| {
    thing_obj
      .values()
      .any(|candidate_value| candidate_value == value)
  })
}

fn branch_target_key(spec: &str) -> &str {
  spec
    .split_once(':')
    .map(|(target, _)| target.trim())
    .unwrap_or(spec)
}

fn branch_source_key(spec: &str) -> &str {
  spec
    .split_once(':')
    .map(|(_, source)| source.trim())
    .unwrap_or(spec)
}

fn map_json_payload(branch_specs: &[String], source: &FlowPayload) -> Option<FlowPayload> {
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
