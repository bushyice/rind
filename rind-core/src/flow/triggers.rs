use serde::{Deserialize, Serialize};

use crate::services::{Service, start_service, stop_service};

use super::*;

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
      &self.lookup::<StateDefinition>(&name).unwrap().0
    } else {
      &self.lookup::<SignalDefinition>(&name).unwrap().0
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

    self.check_triggers(&instance, false);
    self.broadcast(&instance, true, except).ok();

    let entry = self.states.entry(name.clone()).or_insert_with(Vec::new);

    match &instance.payload {
      FlowPayload::String(_) | FlowPayload::Bytes(_) | FlowPayload::None(_) => {
        entry.clear();
        entry.push(instance);
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
          entry.push(instance);
        }
      }
    }

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
          self.check_triggers(branch, true);
          self.broadcast(branch, false, except).ok();
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

      self.check_triggers(&instance, false);
      self.broadcast(&instance, true, except).ok();

      Ok(())
    } else {
      Err(anyhow::anyhow!("Signal trigger validation failed."))
    }
  }

  // pub fn trigger(&self, name: String, payload: FlowPayload, r#type: FlowType) {}

  pub fn open_subs(&self) -> anyhow::Result<()> {
    Ok(())
  }

  pub fn broadcast(
    &mut self,
    instance: &FlowInstance,
    exists: bool,
    except: Option<&Vec<String>>,
  ) -> anyhow::Result<()> {
    println!("{exists} {except:?}");

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
    let mut transports = TRANSPORTS.write().unwrap();

    // Stage 2: Context Buildup
    let empty = &Vec::new();
    let mut ctx = TransportContext::new(&instance.payload, except.unwrap_or(empty), !exists);

    // Stage 3: Actual Trigger
    if let Some(subs) = subs {
      for sub in subs {
        transports
          .get_mut(sub.as_id())
          .unwrap()
          .recv(&mut ctx, &instance, None)?;
      }
    }

    for (_, mut serv) in services {
      transports
        .get_mut(serv.transport.as_ref().unwrap().as_id())
        .unwrap()
        .recv(&mut ctx, &instance, Some(&mut serv))?;
    }

    Ok(())
  }

  pub fn check_triggers(&mut self, trigger: &FlowInstance, remove: bool) {
    let mut to_start_services = Vec::new();
    let mut to_stop_services = Vec::new();

    let state_def = if matches!(trigger.r#type, FlowType::State) {
      &self.lookup::<StateDefinition>(&trigger.name).unwrap().0
    } else {
      &self.lookup::<SignalDefinition>(&trigger.name).unwrap().0
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
          if remove {
            to_stop_services.push(comp_id.clone());
          } else {
            to_start_services.push(comp_id.clone());
          }
        }
      }

      if let Some(stop_on) = &service.stop_on {
        if stop_on.iter().any(|c| check_condition(c, &trigger)) {
          if remove {
            to_start_services.push(comp_id.clone());
          } else {
            to_stop_services.push(comp_id.clone());
          }
        }
      }
    }

    for name in to_stop_services {
      if let Some(service) = self.lookup_mut::<Service>(&name) {
        stop_service(service, false);
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
