use clap::{CommandFactory, Parser};
use owo_colors::OwoColorize;
use rind_ipc::{
  Message, MessageType, Payload,
  send::send_message,
  ser::{ServiceSerialized, UnitItemsSerialized, UnitSerialized},
};
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
}

pub fn handle_message(message: Message) {
  match message.r#type {
    MessageType::Error => {
      println!("{} {}", "Error".on_red().black(), message.payload.unwrap())
    }
    _ => {}
  }
}

fn main() {
  let cli = Cli::parse();

  if cli.list {
    let output: Message = send_message(Message::from_type(MessageType::List).with_payload(
      if let Some(unit) = &cli.unit {
        Payload {
          name: unit.clone(),
          unit_type: rind_ipc::UnitType::Unit,
          force: None,
        }
      } else if let Some(service) = &cli.service {
        Payload {
          name: service.clone(),
          unit_type: rind_ipc::UnitType::Service,
          force: None,
        }
      } else {
        Payload {
          name: "".to_string(),
          unit_type: rind_ipc::UnitType::Unknown,
          force: None,
        }
      },
    ))
    .unwrap();

    // let units_ser = UnitsSerialized::from_string(output.payload.unwrap());
    // let units = units_ser.to_units();

    if let Some(unit_name) = &cli.unit {
      let unit = output.parse_payload::<UnitItemsSerialized>().unwrap();
      print::print_unit(unit_name, &unit);
    } else if let Some(_) = &cli.service {
      let service = output.parse_payload::<ServiceSerialized>().unwrap();
      print::print_service(&service);
    } else {
      let units = output.parse_payload::<Vec<UnitSerialized>>().unwrap();
      print::print_units(&units);
    }
  } else if cli.start {
    if let Some(s) = &cli.service {
      handle!(action!(Start, s.clone(), Service, None));
    }
  } else if cli.stop {
    if let Some(s) = &cli.service {
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
  } else if let Some(logs) = cli.logs {
    let conf = rind_common::config::CONFIG.read().unwrap();
    if let Ok(logs) = rind_common::logger::query_logs(
      conf.logger.log_path.as_str(),
      if logs == "*" { None } else { Some(&logs) },
      None,
      None,
      None,
    ) && logs.len() > 0
    {
      for log in logs {
        rind_common::logger::print_log(&log);
      }
    } else {
      println!("{} {}", "Error".on_red().black(), "No logs found");
    }
  } else {
    Cli::command().print_help().ok();
  }
}
