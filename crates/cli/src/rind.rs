/*
 * TODO: Userspace Update
 * - permissions.
 */

use std::{
  os::unix::process::CommandExt,
  process::{Command, Stdio},
};

use clap::Parser;
use owo_colors::OwoColorize;
use rind_ipc::{
  Message, MessageType,
  payloads::{ListPayload, LogoutPayload, Run0AuthPayload, ServicePayload},
  send::send_message,
  ser::{
    NetworkStatusSerialized, PortStateSerialized, ServiceSerialized, StateSerialized,
    UnitItemsSerialized, UnitSerialized,
  },
};

use crate::print::{print_network, print_ports, print_units};
mod macros;
mod print;

#[derive(clap::Parser)]
#[command(name = "rind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Rust Init Daemon")]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
  Logout,
  Su {
    #[arg(name = "ARGS", trailing_var_arg = true, num_args(1..), allow_hyphen_values = true)]
    args: Vec<String>,
  },
  List {
    #[arg(name = "NAME")]
    name: Option<String>,

    #[arg(short = 'u', long)]
    unit: bool,

    #[arg(short = 's', long)]
    service: bool,

    #[arg(short = 'm', long)]
    mount: bool,

    #[arg(short = 'c', long)]
    state: bool,

    #[arg(short = 'n', long)]
    network: bool,

    #[arg(short = 'p', long)]
    port: bool,

    #[arg(short = 't', long)]
    r#type: Option<String>,
  },

  Start {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,
  },

  Stop {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,

    #[arg(short = 'f', long)]
    force: bool,
  },

  Invoke {
    #[arg(name = "NAME")]
    name: String,

    #[arg(name = "PAYLOAD")]
    payload: String,
  },
}

pub fn report_error(msg: &str, err: impl std::fmt::Display) {
  eprintln!("{} {}: {}", "Error".on_red().black(), msg, err);
}

pub fn handle_parse<T>(result: Result<T, String>, payload: String) -> Option<T> {
  match result {
    Err(e) => {
      if e != "Nothing" {
        report_error(&e, payload);
      }
      None
    }
    Ok(e) => Some(e),
  }
}

pub fn handle_run0_message(args: Vec<String>, message: Message) {
  match &message.r#type {
    MessageType::RequestInput => {
      print!("root password: ");
      use std::io::Write;
      std::io::stdout().flush().unwrap();
      let password = rpassword::read_password().unwrap();
      print!("\r\x1b[2K");
      std::io::stdout().flush().unwrap();
      let payload = Run0AuthPayload {
        password: password.trim().to_string(),
      };
      handle_run0_message(
        args,
        match send_message(Message::from("run0").with(serde_json::to_string(&payload).unwrap())) {
          Ok(m) => m,
          Err(e) => {
            report_error("run0 request failed", e);
            return;
          }
        },
      );
    }
    MessageType::Valid => {
      let mut args = args.into_iter();
      let program = args.next().unwrap();

      let mut command = Command::new(program);

      command
        .args(args)
        .gid(0)
        .uid(0)
        .envs(std::env::vars())
        .current_dir(std::env::current_dir().unwrap())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

      let _ = command.status();
    }
    _ => handle_message(message),
  }
}

pub fn handle_message(message: Message) {
  match message.r#type {
    MessageType::Ok => {
      println!(
        "{} {}",
        "Ok".on_green().black(),
        message.payload.unwrap_or_else(|| "ok".to_string())
      );
    }
    MessageType::Error => {
      println!(
        "{} {}",
        "Error".on_red().black(),
        message
          .payload
          .unwrap_or_else(|| "unknown error".to_string())
      )
    }
    _ => {}
  }
}

fn main() {
  let cli = Cli::parse();

  match cli.command {
    Commands::Su { args } => {
      let output = match send_message(Message::from("run0")) {
        Ok(m) => m,
        Err(e) => {
          report_error("run0 request failed", e);
          return;
        }
      };

      handle_run0_message(args, output);
    }
    Commands::Logout => {
      let username = std::env::var("USER").expect("unknown user");
      let tty = "tty1".to_string();
      handle_send!("logout", &LogoutPayload { username, tty });
    }
    Commands::List {
      unit,
      service,
      mount,
      state,
      network,
      port,
      r#type,
      name,
    } => {
      let name = name.unwrap_or_default();
      let result = send_msg!(
        "list",
        serde_json::to_string(&ListPayload {
          name: name.clone(),
          unit_type: if unit {
            "unit"
          } else if service {
            "service"
          } else if mount {
            "mount"
          } else if state {
            "state"
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
        })
        .unwrap()
      )
      .expect("Failed to send message");

      if unit {
        print::print_unit(
          &name,
          &result
            .parse_payload::<UnitItemsSerialized>()
            .expect("Failed to parse"),
        );
      } else if service {
        print::print_service(
          &result
            .parse_payload::<ServiceSerialized>()
            .expect("Failed to parse"),
        );
      } else if state {
        print::print_state(
          &result
            .parse_payload::<StateSerialized>()
            .expect("Failed to parse"),
        );
      } else if port && network {
        print_ports(
          &result
            .parse_vec_payload::<PortStateSerialized>()
            .expect("Failed to parse"),
        );
      } else if network {
        for status in result
          .parse_vec_payload::<NetworkStatusSerialized>()
          .expect("Failed to parse")
        {
          print_network(&status);
        }
      } else {
        print_units(
          &result
            .parse_vec_payload::<UnitSerialized>()
            .expect("Failed to parse"),
        );
      }
    }
    Commands::Invoke { name, payload } => {
      handle_send_raw!(name.as_str(), payload);
    }
    Commands::Start { name, r#type: _ } => {
      handle_send!("start_service", &ServicePayload { force: None, name });
    }
    Commands::Stop {
      name,
      r#type: _,
      force,
    } => {
      handle_send!(
        "start_service",
        &ServicePayload {
          force: Some(force),
          name
        }
      );
    }
  }
}
