use owo_colors::OwoColorize;
use rind_ipc::ser::{
  IpcListComponent, IpcListPrinter, ServiceSerialized, SocketSerialized, StateSerialized,
  UnitItemsSerialized, UnitSerialized,
};

pub fn print_ipc_list(list: &IpcListComponent) {
  let Some(printer) = &list.printer else {
    for item in &list.components {
      println!("{}", item);
    }
    return;
  };

  match printer.r#type.as_str() {
    "table" => print_as_table(list, printer),
    "list" => print_as_list(list, printer),
    "string" | _ => {
      for item in &list.components {
        println!("{}", item);
      }
    }
  }
}

fn print_as_table(list: &IpcListComponent, printer: &IpcListPrinter) {
  // let mut format_str = String::new();
  // for _ in 0..printer.titles.len() {
  //   format_str.push_str("{:<20} ");
  // }

  // let mut colored_titles = Vec::new();
  // for (i, title) in printer.titles.iter().enumerate() {
  //   let color = printer
  //     .colors
  //     .get(i)
  //     .cloned()
  //     .unwrap_or("white".to_string());
  //   colored_titles.push(colorize_str(title, &color, true));
  // }

  println!(
    "{}",
    printer
      .titles
      .iter()
      .enumerate()
      .map(|(i, t)| colorize_str(
        t,
        printer.colors.get(i).unwrap_or(&"white".to_string()),
        true
      ))
      .collect::<Vec<_>>()
      .join("  ")
  );

  for component in &list.components {
    if let Ok(obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(component) {
      let mut row = Vec::new();
      for (i, key) in printer.keys.iter().enumerate() {
        let val = obj
          .get(key)
          .map(|v| {
            if let Some(s) = v.as_str() {
              s.to_string()
            } else {
              v.to_string()
            }
          })
          .unwrap_or_else(|| "-".to_string());
        let color = printer
          .colors
          .get(i)
          .cloned()
          .unwrap_or("white".to_string());
        row.push(colorize_str(&val, &color, false));
      }
      println!("{}", row.join("  "));
    }
  }
}

fn print_as_list(list: &IpcListComponent, printer: &IpcListPrinter) {
  for component in &list.components {
    if let Ok(obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(component) {
      for (i, key) in printer.keys.iter().enumerate() {
        let title = printer.titles.get(i).cloned().unwrap_or(key.clone());
        let val = obj
          .get(key)
          .map(|v| {
            if let Some(s) = v.as_str() {
              s.to_string()
            } else {
              v.to_string()
            }
          })
          .unwrap_or_else(|| "-".to_string());
        let color = printer
          .colors
          .get(i)
          .cloned()
          .unwrap_or("white".to_string());
        println!(
          "  {}{}: {}",
          if i == 0 {
            "● ".bright_green().to_string()
          } else {
            "  ".to_string()
          },
          title.bold().white(),
          colorize_str(&val, &color, false)
        );
      }
      println!();
    }
  }
}

fn colorize_str(s: &str, color: &str, bold: bool) -> String {
  let mut c = match color {
    "blue" => s.blue().to_string(),
    "green" => s.green().to_string(),
    "yellow" => s.yellow().to_string(),
    "red" => s.red().to_string(),
    "magenta" => s.magenta().to_string(),
    "cyan" => s.cyan().to_string(),
    "white" => s.white().to_string(),

    "on_blue" => s.on_blue().to_string(),
    "on_green" => s.on_green().to_string(),
    "on_yellow" => s.on_yellow().to_string(),
    "on_red" => s.on_red().to_string(),
    "on_magenta" => s.on_magenta().to_string(),
    "on_cyan" => s.on_cyan().to_string(),
    "on_white" => s.on_white().to_string(),
    _ => s.to_string(),
  };
  if bold {
    c = s.bold().to_string();
  }
  c
}

pub fn print_units(units: &[UnitSerialized]) {
  println!(
    "{:<20} {:<10} {:<10} {:<10} {:<15}",
    "Unit".bold().on_cyan().white(),
    "Services".bold().on_green().white(),
    "Sockets".bold().on_blue().white(),
    "Mounts".bold().on_yellow().white(),
    "Flow".bold().on_purple().white(),
  );

  for u in units {
    println!(
      "{:<20} {:<10} {:<10} {:<10} {:<15}",
      u.name.to_string().bold().white(),
      format!("{}/{}", u.active_services, u.services).bright_green(),
      format!("{}/{}", u.active_sockets, u.sockets).bright_blue(),
      format!("{}/{}", u.mounted, u.mounts).bright_yellow(),
      format!("{}/{} | {}", u.active_states, u.states, u.signals).bright_purple(),
    );
  }
}

pub fn print_unit(unit_name: &String, unit: &UnitItemsSerialized) {
  println!("{}", format!("Unit: {}", unit_name).bold().cyan());

  if !unit.services.is_empty() {
    println!("{}", " Services ".on_cyan().bold().white());
    for s in &unit.services {
      println!(
        "  {:<20} {:<10} {:<10} {:<5} {:<}",
        s.name.to_string().bold().white(),
        s.last_state.green(),
        s.after
          .clone()
          .map(|x| x.iter().map(|x| x.to_string()).collect::<Vec<_>>())
          .unwrap_or(vec!["-".to_string()])
          .join(", ")
          .yellow(),
        if s.restart { "R" } else { "-" }.red(),
        s.run.join(" ")
      );
    }
  }

  if !unit.sockets.is_empty() {
    println!("{}", " Sockets ".on_bright_blue().bold().white());
    for s in &unit.sockets {
      println!(
        "  {:<20} {:<10} {:<5} {:<5} {:<}",
        s.name.to_string().bold().white(),
        if s.active {
          "Active".green().to_string()
        } else {
          "Inactive".white().to_string()
        },
        s.triggers.yellow(),
        s.r#type.to_string().blue(),
        s.listen.to_string().yellow(),
      );
    }
  }

  if !unit.states.is_empty() {
    println!("{}", " States ".on_bright_green().bold().white());
    for s in &unit.states {
      println!(
        "  {:<20} {:<5} {:<}",
        s.name.to_string().bold().white(),
        s.instances.len().blue(),
        s.keys.join(" "),
      );
    }
  }

  if !unit.signals.is_empty() {
    println!("{}", " Signals ".on_bright_green().bold().white());
    for s in &unit.signals {
      println!("  {:<}", s.name.to_string().bold().white());
    }
  }

  if !unit.mounts.is_empty() {
    println!("{}", " Mounts ".on_yellow().bold().white());
    for m in &unit.mounts {
      println!(
        "  {:<20} {:<20} {:<10} {:<}",
        m.target.to_string().bold().white(),
        m.source
          .clone()
          .map(|x| x.to_string())
          .unwrap_or("-".into())
          .yellow(),
        m.fstype
          .clone()
          .map(|x| x.to_string())
          .unwrap_or("-".into())
          .cyan(),
        if m.mounted {
          "✓".green().to_string()
        } else {
          "✗".red().to_string()
        }
      );
    }
  }
}

use std::collections::BTreeMap;

pub fn print_state(st: &StateSerialized) {
  let Some(pk) = st.keys.get(0) else {
    println!("{}", st.name.bold());

    for inst in &st.instances {
      println!("{} {inst}", "●".cyan().bold());
    }

    return;
  };

  println!("{}", st.name.to_string().bold().white());

  let mut groups: BTreeMap<String, Vec<&serde_json::Map<String, serde_json::Value>>> =
    BTreeMap::new();

  for inst in &st.instances {
    let Some(obj) = inst.as_object() else {
      continue;
    };

    let key = obj
      .get(&pk.to_string())
      .map(|v| v.to_string())
      .unwrap_or_else(|| "<none>".to_string());

    groups.entry(key).or_default().push(obj);
  }

  for (group_key, items) in groups {
    println!(
      " {} {} {}",
      "●".cyan().bold(),
      pk.bold(),
      group_key.bold().yellow()
    );

    for obj in items {
      for (k, v) in obj {
        if k == pk.as_str() {
          continue;
        }

        println!("   {}: {}", k.bold().white(), value_color(v));
      }

      println!();
    }
  }
}

fn value_color(v: &serde_json::Value) -> String {
  match v {
    serde_json::Value::String(s) => s.green().to_string(),
    serde_json::Value::Number(n) => n.to_string().cyan().to_string(),
    serde_json::Value::Bool(b) => {
      if *b {
        "true".yellow().to_string()
      } else {
        "false".dimmed().to_string()
      }
    }
    serde_json::Value::Null => "null".dimmed().to_string(),
    _ => v.to_string().blue().to_string(), // arrays/objects
  }
}

// pub fn print_states(_st: Vec<StateSerialized>) {}

pub fn print_service(service: &ServiceSerialized) {
  let (dot, state) = match service.last_state.as_str() {
    "Active" => (
      "●".green().bold().to_string(),
      service.last_state.green().bold().to_string(),
    ),
    "Inactive" => (
      "●".white().to_string(),
      service.last_state.white().to_string(),
    ),
    _ => {
      if service.last_state.starts_with("Crashed") || service.last_state.starts_with("Error") {
        (
          "●".bright_red().to_string(),
          service.last_state.bright_red().to_string(),
        )
      } else {
        (
          "●".yellow().to_string(),
          service.last_state.yellow().to_string(),
        )
      }
    }
  };

  println!("{} {}", dot, service.name.bold().white());

  match service.pid {
    Some(pid) => println!(
      "   {}: {} (pid {})",
      "State".bold(),
      state,
      pid.to_string().cyan()
    ),
    None => println!("   {}: {}", "State".bold(), state),
  }

  println!("   {}: {}", "Exec".bold(), service.run.join(", ").cyan());

  // if !service.args.is_empty() {
  //   println!("   {}: {}", "Args".bold(), service.args.join(" ").dimmed());
  // }

  println!(
    "   {}: {}",
    "Restart".bold(),
    if service.restart {
      "yes".yellow().to_string()
    } else {
      "no".dimmed().to_string()
    }
  );

  if let Some(after) = &service.after {
    println!("   {}: {}", "After".bold(), after.join(", ").blue());
  }
}

pub fn print_socket(socket: &SocketSerialized) {
  let (dot, state) = if socket.active {
    (
      "●".green().bold().to_string(),
      "Active".green().bold().to_string(),
    )
  } else {
    ("●".white().to_string(), "Inactive".white().to_string())
  };

  println!("{} {}", dot, socket.name.bold().white());
  println!(
    "   {}: {} (addr {}:{})",
    "State".bold(),
    state,
    socket.r#type.yellow(),
    socket.listen.green()
  );
}
