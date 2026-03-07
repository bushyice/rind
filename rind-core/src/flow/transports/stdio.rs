use std::io::{BufRead, BufReader};

use crate::services::Service;
use rind_common::error::{report_error, rw_write};

use super::*;

#[derive(Default)]
pub struct StdioTransportProtocol;

impl TransportProtocol for StdioTransportProtocol {
  fn init(
    &mut self,
    _options: Vec<String>,
    _ctx: &TransportInitContext,
    service: Option<&mut crate::services::Service>,
  ) {
    let Some(service) = service else {
      return;
    };
    if let Some(child) = service.child.as_mut() {
      start_stdout_listener(service.name.clone(), child);
    }
  }

  fn recv(
    &self,
    ctx: &mut TransportContext,
    instance: &FlowInstance,
    service: Option<&mut Service>,
  ) -> TransportResult {
    if let Some(service) = service {
      if let Some(child) = service.child.as_mut() {
        if let Some(stdin) = child.stdin.as_mut() {
          use std::io::Write;

          // if let FlowPayload::String(msg) = &instance.payload {
          //   writeln!(stdin, "{msg}")?;
          // }

          writeln!(
            stdin,
            "{}",
            serde_json::to_string(&TransportMessage {
              action: ctx.action,
              name: Some(instance.name.clone()),
              payload: Some(instance.payload.clone()),
              r#type: if instance.r#type == FlowType::Signal {
                TransportMessageType::Signal
              } else {
                TransportMessageType::State
              }
            })?
          )?;
        }
      }
    }

    Ok(None)
  }
}

pub fn start_stdout_listener(service_name: String, child: &mut std::process::Child) {
  if let Some(stdout) = child.stdout.take() {
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);

      for line in reader.lines().flatten() {
        let Ok(msg) = serde_json::from_str::<TransportMessage>(&line) else {
          report_error("stdio transport parse error", line);
          continue;
        };

        rw_write(&crate::store::STORE, "store write in stdio listener")
          .handle_message(service_name.clone(), msg);

        // let instance = FlowInstance {
        //   name: service_name.clone(),
        //   payload: FlowPayload::String(line),
        //   r#type: FlowType::Signal,
        // };

        // let _ = sender.send(instance);
      }
    });
  }
}
