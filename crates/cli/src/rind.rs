/*
 * TODO: Userspace Update
 * - permissions.
 */

use std::{
  os::unix::process::CommandExt,
  process::{Command, Stdio},
};

use clap::{CommandFactory, Parser};
use owo_colors::OwoColorize;
use rind_ipc::{
  Message, MessagePayload, MessageType,
  send::send_message,
  ser::{ServiceSerialized, StateSerialized, UnitItemsSerialized, UnitSerialized},
};

use crate::print::{print_network, print_ports};
mod macros;
mod print;

#[derive(clap::Parser)]
#[command(name = "rind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Rust Init Daemon")]
struct Cli {
  #[arg(short = 'L', long)]
  list: bool,

  #[arg(short = 'S', long)]
  start: bool,

  #[arg(short = 'X', long)]
  stop: bool,

  #[arg(long)]
  enable: bool,

  #[arg(long)]
  disable: bool,

  #[arg(long)]
  force: bool,

  #[arg(short = 'u', long, num_args(0..=1), default_missing_value = "")]
  unit: Option<String>,

  #[arg(short = 's', long, num_args(0..=1), default_missing_value = "")]
  service: Option<String>,

  #[arg(short = 'm', long, num_args(0..=1), default_missing_value = "")]
  mount: Option<String>,

  // logs
  #[arg(long, default_missing_value = "*")]
  logs: Option<String>,

  #[arg(short = 'c', long, num_args(0..=1), default_missing_value = "")]
  state: Option<String>,

  #[arg(short = 'e', long, num_args(0..=1), default_missing_value = "")]
  signal: Option<String>,

  #[arg(long)]
  login: bool,

  #[arg(long)]
  logout: bool,

  #[arg(long)]
  user: Option<String>,

  #[arg(long)]
  password: Option<String>,

  #[arg(long)]
  tty: Option<String>,

  #[arg(short = 'E', long, trailing_var_arg = true, num_args(1..), allow_hyphen_values = true)]
  run0: Option<Vec<String>>,

  #[arg(short = 'n', long, num_args(0..=1), default_missing_value = "")]
  network: Option<String>,

  #[arg(long = "na")]
  network_address: bool,

  #[arg(long = "np")]
  network_ports: bool,

  #[arg(long)]
  net_ping: Option<String>,
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
      let payload = rind_ipc::Run0AuthPayload {
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
    MessageType::Ack => {
      println!(
        "{} {}",
        "ACK".on_green().black(),
        message.payload.unwrap_or_else(|| "ok".to_string())
      );
    }
    MessageType::Nack => {
      println!(
        "{} {}",
        "NACK".on_red().black(),
        message
          .payload
          .unwrap_or_else(|| "request failed".to_string())
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

  if cli.list {
    if cli.network.is_some() || cli.network_address {
      let output = match send_message(
        Message::from_type(MessageType::Network).with(
          serde_json::to_string(&rind_ipc::NetworkPayload {
            op: rind_ipc::NetworkOp::Status,
          })
          .unwrap(),
        ),
      ) {
        Ok(m) => m,
        Err(e) => {
          report_error("network-status request failed", e);
          return;
        }
      };
      if let Some(statuses) = handle_parse(
        output
          .parse_vec_payload::<rind_ipc::ser::NetworkStatusSerialized>()
          .ok_or("Invalid Payload".to_string()),
        "".to_string(),
      ) {
        for status in statuses {
          print_network(&status);
        }
      }
      return;
    } else if cli.network_ports {
      let output = match send_message(
        Message::from_type(MessageType::Network).with(
          serde_json::to_string(&rind_ipc::NetworkPayload {
            op: rind_ipc::NetworkOp::Ports,
          })
          .unwrap(),
        ),
      ) {
        Ok(m) => m,
        Err(e) => {
          report_error("network-ports request failed", e);
          return;
        }
      };
      if let Some(ports) = handle_parse(
        output
          .parse_vec_payload::<rind_ipc::ser::PortStateSerialized>()
          .ok_or("Invalid Payload".to_string()),
        "".to_string(),
      ) {
        print_ports(&ports);
      }
      return;
    }

    let output: Message = match send_message(Message::from_type(MessageType::List).with_payload(
      if let Some(unit) = &cli.unit {
        MessagePayload {
          name: unit.clone(),
          unit_type: rind_ipc::UnitType::Unit,
          force: None,
        }
      } else if let Some(service) = &cli.service {
        MessagePayload {
          name: service.clone(),
          unit_type: rind_ipc::UnitType::Service,
          force: None,
        }
      } else if let Some(state) = &cli.state {
        MessagePayload {
          name: state.clone(),
          unit_type: rind_ipc::UnitType::State,
          force: None,
        }
      } else {
        MessagePayload {
          name: "".to_string(),
          unit_type: rind_ipc::UnitType::Unknown,
          force: None,
        }
      },
    )) {
      Ok(message) => message,
      Err(err) => {
        report_error("list request failed", err);
        return;
      }
    };

    let p = output.payload.clone().unwrap_or("".into());

    if let Some(unit_name) = &cli.unit {
      if let Some(unit) = handle_parse(output.parse_payload::<UnitItemsSerialized>(), p) {
        print::print_unit(unit_name, &unit);
      } else {
        report_error("list unit parse failed", "invalid unit payload");
      }
    } else if let Some(_) = &cli.service {
      if let Some(service) = handle_parse(output.parse_payload::<ServiceSerialized>(), p) {
        print::print_service(&service);
      } else {
        report_error("list service parse failed", "invalid service payload");
      }
    } else if let Some(_) = &cli.state {
      if let Some(state) = handle_parse(output.parse_payload::<StateSerialized>(), p) {
        print::print_state(&state);
      } else {
        report_error("list state parse failed", "invalid state payload");
      }
    } else {
      if let Some(units) = handle_parse(output.parse_payload::<Vec<UnitSerialized>>(), p) {
        print::print_units(&units);
      } else {
        report_error("list units parse failed", "invalid units payload");
      }
    }
  } else if cli.start {
    if let Some(n) = &cli.network {
      let output = send_message(
        Message::from_type(MessageType::Network).with(
          serde_json::to_string(&rind_ipc::NetworkPayload {
            op: rind_ipc::NetworkOp::Set {
              iface: n.clone(),
              method: "up".to_string(),
              address: None,
              gateway: None,
            },
          })
          .unwrap(),
        ),
      );
      handle_message(output.unwrap_or_else(|e| Message::nack(format!("Failed: {}", e))));
    } else if let Some(s) = &cli.service {
      handle!(action!(Start, s.clone(), Service, None));
    }
  } else if cli.stop {
    if let Some(n) = &cli.network {
      let output = send_message(
        Message::from_type(MessageType::Network).with(
          serde_json::to_string(&rind_ipc::NetworkPayload {
            op: rind_ipc::NetworkOp::Set {
              iface: n.clone(),
              method: "down".to_string(),
              address: None,
              gateway: None,
            },
          })
          .unwrap(),
        ),
      );
      handle_message(output.unwrap_or_else(|e| Message::nack(format!("Failed: {}", e))));
    } else if let Some(s) = &cli.service {
      handle!(action!(Stop, s.clone(), Service, Some(cli.force)));
    }
  } else if cli.enable {
    if let Some(s) = &cli.service {
      handle!(action!(Enable, s.clone(), Service, None));
    } else if let Some(s) = &cli.mount {
      handle!(action!(Enable, s.clone(), Mount, None));
    } else if let Some(s) = &cli.unit {
      handle!(action!(Enable, s.clone(), Unit, None));
    }
  } else if cli.disable {
    if let Some(s) = &cli.service {
      handle!(action!(Disable, s.clone(), Service, Some(cli.force)));
    } else if let Some(s) = &cli.mount {
      handle!(action!(Disable, s.clone(), Mount, None));
    } else if let Some(s) = &cli.unit {
      handle!(action!(Disable, s.clone(), Unit, None));
    }
  } else if let Some(args) = cli.run0 {
    let output = match send_message(Message::from_type(MessageType::Run0)) {
      Ok(m) => m,
      Err(e) => {
        report_error("run0 request failed", e);
        return;
      }
    };

    handle_run0_message(args, output);
  } else if cli.login || cli.logout {
    let username = cli
      .user
      .unwrap_or_else(|| std::env::var("USER").expect("unknown user"));
    let tty = cli.tty.unwrap_or_else(|| "tty1".to_string());

    if cli.login {
      let payload = rind_ipc::LoginPayload {
        username,
        password: cli.password.clone(),
        tty,
      };
      let output = match send_message(
        Message::from_type(MessageType::Login).with(serde_json::to_string(&payload).unwrap()),
      ) {
        Ok(m) => m,
        Err(e) => {
          report_error("login request failed", e);
          return;
        }
      };
      handle_message(output);
    } else {
      let payload = rind_ipc::LogoutPayload { username, tty };
      let output = match send_message(
        Message::from_type(MessageType::Logout).with(serde_json::to_string(&payload).unwrap()),
      ) {
        Ok(m) => m,
        Err(e) => {
          report_error("logout request failed", e);
          return;
        }
      };
      handle_message(output);
    }
  } else if let Some(target) = &cli.net_ping {
    let _ = std::process::Command::new("ping")
      .arg("-c")
      .arg("4")
      .arg(target)
      .spawn()
      .and_then(|mut c| c.wait());
  } else {
    Cli::command().print_help().ok();
  }
}
