use clap::Parser;
use rind_ipc::{Message, MessageType, send::send_message, ser::UnitsSerialized};

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

  #[arg(short = 'u', long, num_args(0..=1), default_missing_value = "")]
  unit: Option<String>,

  #[arg(short = 's', long, num_args(0..=1), default_missing_value = "")]
  service: Option<String>,
}

fn main() {
  let cli = Cli::parse();

  if cli.list {
    let output: Message = send_message(Message::from_type(MessageType::List)).unwrap();

    let units_ser = UnitsSerialized::from_string(output.payload.unwrap());
    let units = units_ser.to_units();

    if let Some(unit) = cli.unit {
    } else if let Some(s) = cli.service {
    } else {
      for (name, unit) in units.each() {
        println!(
          "{}: {} services, {} mounts",
          name.to_string(),
          unit.service.as_ref().map_or(0, |x| x.len()),
          unit.mount.as_ref().map_or(0, |x| x.len())
        );
      }
    }
  }
}
