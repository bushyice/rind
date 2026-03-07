use clap::{CommandFactory, Parser};
use owo_colors::OwoColorize;
use rind_common::error::{install_panic_handler, report_error, rw_read};
use rind_ipc::{
  Message, MessagePayload, MessageType,
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
  install_panic_handler("cli");
  let cli = Cli::parse();

  if cli.list {
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

    // let units_ser = UnitsSerialized::from_string(output.payload.unwrap());
    // let units = units_ser.to_units();

    if let Some(unit_name) = &cli.unit {
      if let Some(unit) = output.parse_payload::<UnitItemsSerialized>() {
        print::print_unit(unit_name, &unit);
      } else {
        report_error("list unit parse failed", "invalid unit payload");
      }
    } else if let Some(_) = &cli.service {
      if let Some(service) = output.parse_payload::<ServiceSerialized>() {
        print::print_service(&service);
      } else {
        report_error("list service parse failed", "invalid service payload");
      }
    } else {
      if let Some(units) = output.parse_payload::<Vec<UnitSerialized>>() {
        print::print_units(&units);
      } else {
        report_error("list units parse failed", "invalid units payload");
      }
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
    let conf = rw_read(&rind_common::config::CONFIG, "config read in cli logs");
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
