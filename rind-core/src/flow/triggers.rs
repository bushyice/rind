use serde::{Deserialize, Serialize};

use crate::{
  lookup::LookUpComponent,
  services::{Service, start_service, stop_service},
};

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
  pub fn check_flow(&self, r#type: FlowType, name: &String, payload: &Option<FlowPayload>) -> bool {
    let flowdef = if matches!(r#type, FlowType::State) {
      &self.lookup::<StateDefinition>(&name).unwrap().0
    } else {
      &self.lookup::<SignalDefinition>(&name).unwrap().0
    };
    let Some(p) = payload else {
      return true;
    };
    if match p {
      FlowPayload::Bytes(_) => matches!(flowdef.payload, FlowPayloadType::Bytes),
      FlowPayload::String(_) => matches!(flowdef.payload, FlowPayloadType::String),
      FlowPayload::Json(_) => matches!(flowdef.payload, FlowPayloadType::Json),
    } {
      true
    } else {
      false
    }
  }

  pub fn set_state(&mut self, name: String, payload: Option<FlowPayload>) -> anyhow::Result<()> {
    if self.check_flow(FlowType::State, &name, &payload) {
      let instance = FlowInstance {
        name: name.clone(),
        payload: if let Some(p) = payload {
          p
        } else {
          FlowPayload::String("".to_string())
        },
        r#type: FlowType::State,
      };

      self.check_triggers(&instance);

      let entry = self.states.entry(name.clone()).or_insert_with(Vec::new);
      entry.push(instance);
      Ok(())
    } else {
      Err(anyhow::anyhow!("State trigger validation failed."))
    }
  }

  pub fn remove_state(&mut self, name: &str, filter: Option<FlowMatchOperation>) {
    if let Some(filter) = &filter {
      let Some(branches) = self.states.get_mut(name) else {
        return;
      };

      branches.retain(|branch| !match_operation(filter, &branch.payload));
    } else {
      self.states.remove(name);
    }
  }

  pub fn emit_signal(&mut self, name: String, payload: Option<FlowPayload>) -> anyhow::Result<()> {
    if self.check_flow(FlowType::Signal, &name, &payload) {
      let instance = FlowInstance {
        name: name.clone(),
        payload: if let Some(p) = payload {
          p
        } else {
          FlowPayload::String("".to_string())
        },
        r#type: FlowType::State,
      };

      self.check_triggers(&instance);

      Ok(())
    } else {
      Err(anyhow::anyhow!("Signal trigger validation failed."))
    }
  }

  pub fn check_triggers(&mut self, trigger: &FlowInstance) {
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
          to_start_services.push(comp_id.clone());
        }
      }
      if let Some(stop_on) = &service.stop_on {
        if stop_on.iter().any(|c| check_condition(c, &trigger)) {
          to_stop_services.push(comp_id.clone());
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

fn payload_to_string(payload: &FlowPayload) -> String {
  match payload {
    FlowPayload::String(s) => s.clone(),
    FlowPayload::Json(s) => s.to_string(),
    // FIX: Proper error handling
    FlowPayload::Bytes(s) => String::from_utf8(s.clone()).unwrap_or("".to_string()),
  }
}

fn value_to_vec_string(value: &serde_json::Value) -> Vec<String> {
  match value {
    serde_json::Value::Array(arr) => arr
      .into_iter()
      .filter_map(|v| match v {
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
      })
      .collect(),
    _ => vec!["".to_string()],
  }
}

fn payload_contains(contains: &String, payload: &FlowPayload) -> bool {
  match payload {
    FlowPayload::String(s) => s.contains(contains),
    FlowPayload::Json(s) => value_to_vec_string(s).contains(contains),
    // TODO: Add a binary contains checker
    FlowPayload::Bytes(_) => false,
  }
}

fn match_operation(matcher: &FlowMatchOperation, payload: &FlowPayload) -> bool {
  match matcher {
    FlowMatchOperation::Eq(s) => payload_to_string(payload) == *s,
    FlowMatchOperation::Options {
      binary,
      contains,
      r#as,
    } => {
      if let Some(true) = binary {
        matches!(payload, FlowPayload::Bytes(_))
      } else if let Some(contains) = contains {
        payload_contains(contains, payload)
      } else if let Some(filter) = r#as {
        match payload {
          FlowPayload::Json(payload) => subset_match(filter, payload),
          _ => false,
        }
      } else {
        false
      }
    }
  }
}
