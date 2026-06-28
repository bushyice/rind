use clap::Parser;
use rind_ipc::{Message, MessageType, send::send_message, ser::ser_to_vec};

use crate::{apply_scope_name, handle_message, handle_send_raw, send_msg};

#[derive(Parser)]
#[command(name = "sysinvoke")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
pub struct Cli {
  #[arg(name = "NAME")]
  name: String,

  #[arg(name = "PAYLOAD")]
  payload: String,

  #[arg(long)]
  scope: Option<String>,
}

pub fn main() {
  let cli = Cli::parse();

  let mut v: serde_json::Value =
    serde_json::from_str(&cli.payload).unwrap_or(serde_json::Value::String(cli.payload));

  if let Some(scope) = cli.scope.filter(|s| !s.is_empty() && s != "static") {
    if let Some(obj) = v.as_object_mut()
      && let Some(n) = obj.get("name").and_then(|x| x.as_str())
    {
      obj.insert(
        "name".to_string(),
        serde_json::Value::String(apply_scope_name(n, Some(&scope))),
      );
    }
  }

  handle_send_raw!(cli.name.as_str(), ser_to_vec(&v, false));
}
