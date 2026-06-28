use clap::Parser;
use rind_ipc::{Message, MessageType, send::send_message, ser::ser_to_vec};

use crate::{handle_message, handle_send, handle_send_raw, send_msg};

#[derive(Parser)]
#[command(name = "sysman")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
pub struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
  Start {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,

    #[arg(short = 'p', long)]
    persist: bool,

    #[arg(long)]
    scope: Option<String>,
  },
  Stop {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,

    #[arg(short = 'f', long)]
    force: bool,

    #[arg(short = 'p', long)]
    persist: bool,

    #[arg(long)]
    scope: Option<String>,
  },
  Show {
    #[arg(name = "NAME")]
    name: Option<String>,

    #[arg(short = 'u', long)]
    unit: bool,

    #[arg(short = 's', long)]
    service: bool,

    #[arg(short = 'x', long)]
    socket: bool,

    #[arg(short = 'm', long)]
    mount: bool,

    #[arg(short = 'c', long)]
    facet: bool,

    #[arg(short = 'n', long)]
    network: bool,

    #[arg(short = 'p', long)]
    port: bool,

    #[arg(short = 't', long)]
    r#type: Option<String>,

    #[arg(long)]
    scope: Option<String>,
  },
  Scope {
    #[command(subcommand)]
    action: ScopeCommand,
  },
}

#[derive(clap::Subcommand)]
enum ScopeCommand {
  Create {
    #[arg(name = "NAME")]
    name: String,

    #[arg(long)]
    lifetime_state: Option<String>,

    #[arg(long = "attr", value_name = "KEY=VALUE")]
    attrs: Vec<String>,

    #[arg(long)]
    user: Option<String>,
  },
  Destroy {
    #[arg(name = "NAME")]
    name: String,
  },
}

fn parse_scope_attrs(
  attrs: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
  let mut out = std::collections::HashMap::new();
  for attr in attrs {
    let Some((k, v)) = attr.split_once('=') else {
      return Err(format!("invalid --attr value '{attr}', expected KEY=VALUE"));
    };
    let key = k.trim();
    if key.is_empty() {
      return Err(format!(
        "invalid --attr value '{attr}', key cannot be empty"
      ));
    }
    out.insert(key.to_string(), v.trim().to_string());
  }
  Ok(out)
}

pub fn main() {
  let cli = Cli::parse();

  match cli.command {
    Command::Start {
      name,
      r#type,
      persist,
      scope,
    } => {
      let name = crate::apply_scope_name(&name, scope.as_deref());
      let action = if r#type == "socket" || r#type == "soc" {
        "start_socket"
      } else if r#type == "service" || r#type == "svc" {
        "start_service"
      } else {
        "start"
      };
      handle_send!(
        action,
        &rind_ipc::payloads::SSPayload {
          force: false,
          name,
          persist,
          unit_type: r#type
        }
      );
    }
    Command::Stop {
      name,
      r#type,
      force,
      persist,
      scope,
    } => {
      let name = crate::apply_scope_name(&name, scope.as_deref());
      let action = if r#type == "socket" || r#type == "soc" {
        "stop_socket"
      } else if r#type == "service" || r#type == "svc" {
        "stop_service"
      } else {
        "stop"
      };
      handle_send!(
        action,
        &rind_ipc::payloads::SSPayload {
          force,
          name,
          persist,
          unit_type: r#type
        }
      );
    }
    Command::Show {
      name,
      unit,
      service,
      socket,
      mount,
      facet,
      network,
      port,
      mut r#type,
      scope,
    } => {
      let name = crate::apply_scope_name(&name.unwrap_or_default(), scope.as_deref());
      let result = send_msg!(
        "show",
        ser_to_vec(
          &rind_ipc::payloads::ListPayload {
            name: name.clone().into(),
            scope,
            unit_type: if unit {
              "unit"
            } else if service {
              "service"
            } else if mount {
              "mount"
            } else if facet {
              "facet"
            } else if socket {
              "socket"
            } else if port && network {
              "netport"
            } else if network {
              "netiface"
            } else if r#type.is_some() {
              r#type.as_ref().unwrap()
            } else {
              "unknown"
            }
            .into(),
          },
          false
        )
      )
      .expect("Failed to send message");

      if r#type.is_none() && port && network {
        r#type = Some("netports".to_string());
      } else if r#type.is_none() && network {
        r#type = Some("netiface".to_string());
      }

      if matches!(result.r#type, MessageType::Error) {
        handle_message(result);
        return;
      }

      use rind_ipc::ser::{
        FacetSerialized, ServiceSerialized, SocketSerialized, UnitItemsSerialized, UnitSerialized,
      };

      if unit {
        crate::print::print_unit(
          &name,
          &result
            .parse_payload::<UnitItemsSerialized>()
            .expect("Failed to parse"),
        );
      } else if service {
        crate::print::print_service(
          &result
            .parse_payload::<ServiceSerialized>()
            .expect("Failed to parse"),
        );

        #[cfg(not(feature = "no-syslogs"))]
        {
          let entries = crate::applets::syslogs::read_entries_once(
            &crate::applets::syslogs::get_logs_dir(),
            &crate::applets::syslogs::LogQuery {
              level: None,
              target: None,
              message: None,
              exact: false,
              since: crate::applets::syslogs::current_boot_start_unix().ok(),
              fields: vec![("service".to_string(), name.clone())],
            },
            10,
          );

          if !entries.is_empty() {
            println!("== logs ==");
          }

          for entry in &entries {
            if let Err(err) = crate::applets::syslogs::write_log_entry(
              &mut crate::applets::syslogs::OutputSink::stdout(),
              entry,
            ) {
              crate::report_error("logs print failed", err);
              return;
            }
          }
        }
      } else if facet {
        crate::print::print_state(
          &result
            .parse_payload::<FacetSerialized>()
            .expect("Failed to parse"),
        );
      } else if socket {
        crate::print::print_socket(
          &result
            .parse_payload::<SocketSerialized>()
            .expect("Failed to parse"),
        );
      } else if r#type.is_some()
        && let Some(ref ty) = r#type
        && !ty.is_empty()
        && ty != "unknown"
      {
        if let Ok(list) = result.parse_payload::<rind_ipc::ser::IpcListComponent>() {
          crate::print::print_ipc_list(&list);
        } else {
          println!(
            "{}",
            String::from_utf8_lossy(&result.payload.unwrap_or_default())
          );
        }
      } else {
        crate::print::print_units(
          &result
            .parse_vec_payload::<UnitSerialized>()
            .expect("Failed to parse"),
        );
      }
    }
    Command::Scope { action } => match action {
      ScopeCommand::Create {
        name,
        lifetime_state,
        attrs,
        user,
      } => {
        let mut attributes = match parse_scope_attrs(&attrs) {
          Ok(v) => v,
          Err(err) => {
            crate::report_error("invalid scope attributes", err);
            return;
          }
        };
        if let Some(user) = user {
          attributes.insert("user".to_string(), user);
        }
        handle_send!(
          "create_scope",
          &rind_ipc::payloads::ScopeCreatePayload {
            scope: name,
            lifetime_state,
            attributes,
          }
        );
      }
      ScopeCommand::Destroy { name } => {
        handle_send!(
          "destroy_scope",
          &rind_ipc::payloads::ScopeDestroyPayload { scope: name }
        );
      }
    },
  }
}
