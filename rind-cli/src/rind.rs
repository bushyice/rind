use clap::{CommandFactory, Parser};
use rind_ipc::{
  Message, MessageType, Payload, Service, ServiceState, send::send_message, ser::UnitsSerialized,
};

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
}

fn main() {
  let cli = Cli::parse();

  if cli.list {
    let output: Message = send_message(Message::from_type(MessageType::List)).unwrap();

    let units_ser = UnitsSerialized::from_string(output.payload.unwrap());
    let units = units_ser.to_units();

    if let Some(_unit) = &cli.unit {
    } else if let Some(_s) = &cli.service {
    } else {
      for (name, unit) in units.each() {
        println!(
          "{}: {} services({} running, {} crashed), {} mounts",
          name.to_string(),
          unit.service.as_ref().map_or(0, |x| x.len()),
          unit.service.as_ref().map_or(0, |x| x
            .iter()
            .filter(|x| matches!(x.last_state, ServiceState::Active))
            .collect::<Vec<&Service>>()
            .len()),
          unit.service.as_ref().map_or(0, |x| x
            .iter()
            .filter(|x| matches!(x.last_state, ServiceState::Error(_)))
            .collect::<Vec<&Service>>()
            .len()),
          unit.mount.as_ref().map_or(0, |x| x.len())
        );
      }
    }
  } else if cli.start {
    if let Some(s) = &cli.service {
      send_message(
        Message::from_type(MessageType::Start).with_payload(Payload {
          force: None,
          name: s.clone(),
          unit_type: rind_ipc::UnitType::Service,
        }),
      )
      .unwrap();
    }
  } else if cli.stop {
    if let Some(s) = &cli.service {
      send_message(Message::from_type(MessageType::Stop).with_payload(Payload {
        force: Some(cli.force),
        name: s.clone(),
        unit_type: rind_ipc::UnitType::Service,
      }))
      .unwrap();
    }
  } else if cli.enable {
    if let Some(s) = &cli.service {
      send_message(
        Message::from_type(MessageType::Enable).with_payload(Payload {
          force: None,
          name: s.clone(),
          unit_type: rind_ipc::UnitType::Service,
        }),
      )
      .unwrap();
    }
  } else if cli.disable {
    if let Some(s) = &cli.service {
      send_message(
        Message::from_type(MessageType::Disable).with_payload(Payload {
          force: None,
          name: s.clone(),
          unit_type: rind_ipc::UnitType::Service,
        }),
      )
      .unwrap();
    }
  } else {
    Cli::command().print_help().ok();
  }
}
