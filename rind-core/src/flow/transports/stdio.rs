use std::io::{BufRead, BufReader};

use crate::services::Service;

use super::*;

#[derive(Default)]
pub struct StdioTransportProtocol;

impl TransportProtocol for StdioTransportProtocol {
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
              action: if ctx.remove {
                TransportMessageAction::Remove
              } else {
                TransportMessageAction::Set
              },
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

  fn init(&mut self, _options: Vec<String>, service: Option<&mut crate::services::Service>) {
    if let Some(service) = service {
      start_stdout_listener(service.name.clone(), service.child.as_mut().unwrap());
    }
  }
}

pub fn start_stdout_listener(service_name: String, child: &mut std::process::Child) {
  if let Some(stdout) = child.stdout.take() {
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);

      for line in reader.lines().flatten() {
        let msg: TransportMessage = serde_json::from_str(&line).unwrap();

        crate::store::STORE
          .write()
          .unwrap()
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
