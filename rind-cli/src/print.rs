use owo_colors::OwoColorize;
use rind_ipc::ser::{ServiceSerialized, StateSerialized, UnitItemsSerialized, UnitSerialized};

pub fn print_units(units: &[UnitSerialized]) {
  println!(
    "{:<20} {:<10} {:<15} {:<10} {:<10}",
    "Unit".bold().on_cyan().white(),
    "Services".bold().on_green().white(),
    "Active".bold().on_green().white(),
    "Mounts".bold().on_yellow().white(),
    "Mounted".bold().on_yellow().white()
  );

  for u in units {
    println!(
      "{:<20} {:<10} {:<15} {:<10} {:<10}",
      u.name.bold().white(),
      u.services.to_string().green(),
      u.active_services.to_string().green(),
      u.mounts.to_string().yellow(),
      u.mounted.to_string().yellow()
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
        s.name.bold().white(),
        s.last_state.green(),
        s.after
          .clone()
          .unwrap_or(vec!["-".to_string()])
          .join(", ")
          .yellow(),
        if s.restart { "R" } else { "-" }.red(),
        s.run.join(" ")
      );
    }
  }

  if !unit.mounts.is_empty() {
    println!("{}", " Mounts ".on_yellow().bold().white());
    for m in &unit.mounts {
      println!(
        "  {:<20} {:<20} {:<10} {:<}",
        m.target.bold().white(),
        m.source.clone().unwrap_or("-".to_string()).yellow(),
        m.fstype.clone().unwrap_or("-".to_string()).cyan(),
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

  println!("{}", st.name.bold().white());

  let mut groups: BTreeMap<String, Vec<&serde_json::Map<String, serde_json::Value>>> =
    BTreeMap::new();

  for inst in &st.instances {
    let Some(obj) = inst.as_object() else {
      continue;
    };

    let key = obj
      .get(pk)
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
        if k == pk {
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
