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
        s.args.join(" ")
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

pub fn print_state(st: &StateSerialized) {
  println!("{}: {}", st.name, st.instances)
}

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

  println!("   {}: {}", "Exec".bold(), service.exec.cyan());

  if !service.args.is_empty() {
    println!("   {}: {}", "Args".bold(), service.args.join(" ").dimmed());
  }

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
