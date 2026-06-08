use std::collections::HashMap;
use std::path::Path;

use rind_cfg::loader::RegisterLoader;
use rind_core::prelude::*;
use rind_flow::{FlowFacet, FlowFacetMetadata, FlowItem, FlowPayloadType};
use rind_plugins::prelude::*;
use rind_primitives::mounts::{Mount, MountMetadata};
use rind_services::services::{
  RestartPolicy, RunOption, RunOptions, Service, ServiceCgroup, ServiceMetadata, ServiceSpace,
};
use rind_services::sockets::{Socket, SocketMetadata, SocketType};
use rind_services::timers::{Timer, TimerMetadata};

plugin_extensible!(EXTENSIONS);

#[derive(Default, Debug)]
struct IniFile {
  sections: Vec<(String, HashMap<String, Vec<String>>)>,
}

fn parse_ini(content: &str) -> IniFile {
  let mut file = IniFile::default();
  let mut current: Option<String> = None;
  let mut current_map: HashMap<String, Vec<String>> = HashMap::new();
  let mut pending = String::new();

  for raw_line in content.lines() {
    let line = if pending.is_empty() {
      raw_line.to_string()
    } else {
      format!("{pending}{raw_line}")
    };

    let stripped = strip_comment(line.trim_start());

    if stripped.is_empty() {
      pending.clear();
      continue;
    }

    if is_continuation(stripped) {
      pending = strip_trailing_backslash(stripped);
      pending.push(' ');
      continue;
    }
    pending.clear();

    if let Some(section) = parse_section_header(stripped) {
      if let Some(name) = current.take() {
        file.sections.push((name, std::mem::take(&mut current_map)));
      }
      current = Some(section);
      continue;
    }

    let Some((key, value)) = split_kv(stripped) else {
      continue;
    };

    let key = key.trim().to_string();
    let value = value.trim().to_string();
    if !value.is_empty() {
      current_map.entry(key).or_default().push(value);
    }
  }

  if let Some(name) = current {
    file.sections.push((name, current_map));
  }

  file
}

fn is_continuation(line: &str) -> bool {
  line.ends_with('\\') && !line.ends_with("\\\\")
}

fn strip_trailing_backslash(line: &str) -> String {
  let trimmed = line.trim_end();
  let trimmed = trimmed.trim_end_matches('\\');
  trimmed.to_string()
}

fn strip_comment(line: &str) -> &str {
  let bytes = line.as_bytes();
  let mut in_single = false;
  let mut in_double = false;
  for (i, &b) in bytes.iter().enumerate() {
    match b {
      b'\'' if !in_double => in_single = !in_single,
      b'"' if !in_single => in_double = !in_double,
      b'#' if !in_single && !in_double => return &line[..i],
      b';' if !in_single && !in_double => return &line[..i],
      _ => {}
    }
  }
  line
}

fn parse_section_header(line: &str) -> Option<String> {
  let trimmed = line.trim();
  if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
    Some(trimmed[1..trimmed.len() - 1].trim().to_string())
  } else {
    None
  }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
  line.find('=').map(|i| (&line[..i], &line[i + 1..]))
}

fn first_value<'a>(section: &'a HashMap<String, Vec<String>>, key: &str) -> Option<&'a str> {
  section.get(key).and_then(|v| v.first().map(|s| s.as_str()))
}

fn all_values(section: &HashMap<String, Vec<String>>, key: &str) -> Vec<String> {
  section.get(key).cloned().unwrap_or_default()
}

fn split_whitespace(value: &str) -> Vec<String> {
  value
    .split_whitespace()
    .map(|s| s.trim_end_matches(',').to_string())
    .filter(|s| !s.is_empty())
    .collect()
}

fn systemd_env_value(value: &str) -> HashMap<String, String> {
  let mut env = HashMap::new();
  for entry in split_whitespace(value) {
    if let Some((k, v)) = entry.split_once('=') {
      env.insert(k.to_string(), v.to_string());
    } else {
      env.insert(entry, String::new());
    }
  }
  env
}

fn systemd_exec(value: &str) -> Option<(String, Vec<String>)> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return None;
  }

  let mut tokens: Vec<String> = Vec::new();
  let mut current = String::new();
  let mut in_single = false;
  let mut in_double = false;
  let mut had_token = false;
  let mut chars = trimmed.chars().peekable();

  while let Some(&c) = chars.peek() {
    if c.is_whitespace() && !in_single && !in_double {
      if had_token {
        tokens.push(std::mem::take(&mut current));
        had_token = false;
      }
      chars.next();
      continue;
    }
    had_token = true;
    chars.next();

    match c {
      '\'' if !in_double => {
        in_single = !in_single;
      }
      '"' if !in_single => {
        in_double = !in_double;
      }
      '\\' if in_double => {
        if let Some(&next) = chars.peek() {
          current.push(next);
          chars.next();
        }
      }
      _ => current.push(c),
    }
  }

  if had_token || !current.is_empty() {
    tokens.push(current);
  }

  let mut iter = tokens.into_iter();
  let exec = iter.next()?;
  let args: Vec<String> = iter.collect();
  Some((exec, args))
}

fn parse_time_spec(value: &str) -> Option<String> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return None;
  }
  let (num_str, unit) = trimmed.split_at(trimmed.len() - 1);
  let n: u64 = match num_str.parse() {
    Ok(v) => v,
    Err(_) => return None,
  };
  match unit {
    "s" | "S" => Some(format!("{n}s")),
    "m" | "M" => Some(format!("{n}m")),
    "h" | "H" => Some(format!("{n}h")),
    "d" | "D" => Some(format!("{n}d")),
    "w" | "W" => Some(format!("{}d", n * 7)),
    "y" | "Y" => Some(format!("{}d", n * 365)),
    "u" | "U" => None,
    _ => Some(trimmed.to_string()),
  }
}

fn parse_on_calendar(value: &str) -> Option<String> {
  let value = value.trim();
  if value.is_empty() {
    return None;
  }
  let lower = value.to_ascii_lowercase();

  for (prefix, dur) in [
    ("minutely", "1m"),
    ("hourly", "1h"),
    ("daily", "1d"),
    ("weekly", "7d"),
    ("monthly", "30d"),
    ("quarterly", "90d"),
    ("semi-annually", "180d"),
    ("yearly", "365d"),
    ("annually", "365d"),
  ] {
    if let Some(rest) = lower.strip_prefix(prefix) {
      if rest.trim().is_empty() {
        return Some(dur.to_string());
      }
    }
  }
  None
}

fn timer_duration_from(tmr: &HashMap<String, Vec<String>>) -> Option<String> {
  for key in [
    "OnCalendar",
    "OnBootSec",
    "OnUnitActiveSec",
    "OnUnitInactiveSec",
    "OnActiveSec",
  ] {
    if let Some(v) = first_value(tmr, key) {
      if let Some(d) = parse_on_calendar(v) {
        return Some(d);
      }
      if let Some(d) = parse_time_spec(v) {
        return Some(d);
      }
    }
  }
  None
}

fn target_to_facet(target: &str) -> String {
  match target.trim_end_matches(".target") {
    "network-online" => "net:online!".to_string(),
    "network" => "net:configured!".to_string(),
    "network-pre" => "net:configured!".to_string(),
    "multi-user" => "rind:up!".to_string(),
    "graphical" => "rind:graphical".to_string(),
    "sockets" => "rind:sockets".to_string(),
    "timers" => "rind:timers".to_string(),
    "paths" => "rind:paths".to_string(),
    "basic" => "rind:basic".to_string(),
    "sysinit" => "rind:sysinit".to_string(),
    "rescue" => "rind:rescue".to_string(),
    "emergency" => "rind:emergency".to_string(),
    other => format!("systemd:{other}"),
  }
}

fn target_ref(value: &str) -> Option<String> {
  let trimmed = value.trim();
  if !trimmed.ends_with(".target") {
    return None;
  }
  let resolved = target_to_facet(trimmed);
  if resolved.is_empty() {
    None
  } else {
    Some(resolved)
  }
}

fn collect_dependencies(unit: Option<&HashMap<String, Vec<String>>>) -> (Vec<String>, Vec<String>) {
  let mut after: Vec<String> = Vec::new();
  let mut wanted: Vec<String> = Vec::new();
  let Some(map) = unit else {
    return (after, wanted);
  };

  for key in ["After", "Requires", "Wants"] {
    for v in all_values(map, key) {
      for token in split_whitespace(&v) {
        if token.is_empty() {
          continue;
        }
        if let Some(facet) = target_ref(&token) {
          wanted.push(facet);
        } else {
          after.push(token.to_string());
        }
      }
    }
  }

  for key in ["WantedBy", "RequiredBy"] {
    for v in all_values(map, key) {
      for token in split_whitespace(&v) {
        if let Some(facet) = target_ref(&token) {
          wanted.push(facet);
        }
      }
    }
  }

  (after, wanted)
}

fn load_into(name: &str, ini: &IniFile, metadata: &mut Metadata) {
  let mut section_map: HashMap<String, &HashMap<String, Vec<String>>> = HashMap::new();
  for (sname, smap) in &ini.sections {
    section_map.insert(sname.to_ascii_lowercase(), smap);
  }

  let unit = section_map.get("unit").copied();
  let (after_list, wanted_facets) = collect_dependencies(unit);

  if let Some(svc) = section_map.get("service") {
    metadata.insert::<Service>(
      name,
      build_service_meta(name, svc, &after_list, &wanted_facets),
    );
  }
  if let Some(sock) = section_map.get("socket") {
    metadata.insert::<Socket>(
      name,
      build_socket_meta(name, sock, &after_list, &wanted_facets),
    );
  }
  if let Some(tmr) = section_map.get("timer") {
    if let Some(meta) = build_timer_meta(name, tmr, &after_list) {
      metadata.insert::<Timer>(name, meta);
    }
  }
  if let Some(mnt) = section_map.get("mount") {
    metadata.insert::<Mount>(name, build_mount_meta(name, mnt, &after_list));
  }

  let is_target_file = section_map.get("service").is_none()
    && section_map.get("socket").is_none()
    && section_map.get("timer").is_none()
    && section_map.get("mount").is_none()
    && section_map.get("automount").is_none()
    && unit.is_some();

  if is_target_file {
    let referenced_facets = referenced_targets_from(unit, &wanted_facets);
    let mut emit_facets = referenced_facets;
    let own_facet = target_to_facet(name);
    if !own_facet.is_empty() && !emit_facets.contains(&own_facet) {
      emit_facets.push(own_facet);
    }
    for facet_name in &emit_facets {
      metadata.insert::<FlowFacet>(name, build_facet_meta(facet_name));
    }
  }
}

fn referenced_targets_from(
  unit: Option<&HashMap<String, Vec<String>>>,
  wanted: &[String],
) -> Vec<String> {
  let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
  for f in wanted {
    if !f.is_empty() {
      set.insert(f.clone());
    }
  }
  if let Some(map) = unit {
    for key in ["After", "Requires", "Wants", "WantedBy", "RequiredBy"] {
      for v in all_values(map, key) {
        for token in split_whitespace(&v) {
          if let Some(facet) = target_ref(&token) {
            set.insert(facet);
          }
        }
      }
    }
  }
  set.into_iter().collect()
}

fn build_facet_meta(name: &str) -> FlowFacetMetadata {
  FlowFacetMetadata {
    name: Ustr::from(name),
    payload: FlowPayloadType::Json,
    ..Default::default()
  }
}

fn build_service_meta(
  name: &str,
  svc: &HashMap<String, Vec<String>>,
  after: &[String],
  start_on: &[String],
) -> ServiceMetadata {
  let mut meta = ServiceMetadata::default();
  meta.name = Ustr::from(name);

  if let Some(exec) = first_value(svc, "ExecStart").and_then(systemd_exec) {
    let (exec, args) = exec;
    let env = collect_env(svc);
    let run = RunOption {
      exec: Ustr::from(&exec),
      args: args.iter().map(|a| Ustr::from(a)).collect(),
      env: if env.is_empty() {
        None
      } else {
        Some(
          env
            .into_iter()
            .map(|(k, v)| (Ustr::from(k), Ustr::from(v)))
            .collect(),
        )
      },
      ..Default::default()
    };
    meta.run = RunOptions::One(run);
  }

  meta.restart = first_value(svc, "Restart").and_then(systemd_restart_to_policy);

  if let Some(wd) = first_value(svc, "WorkingDirectory") {
    meta.working_dir = Some(Ustr::from(wd));
  }

  if let Some(user) = first_value(svc, "User") {
    if user != "root" {
      meta.space = ServiceSpace::UserSelective {
        user: Ustr::from(user),
      };
    }
  }

  let mut cgroup = ServiceCgroup::default();
  let mut cgroup_set = false;
  if let Some(mem) = first_value(svc, "MemoryMax") {
    if !mem.is_empty() && mem != "infinity" {
      cgroup.memory_max = Some(Ustr::from(mem));
      cgroup_set = true;
    }
  }
  if let Some(cpu) = first_value(svc, "CPUQuota") {
    cgroup.cpu_max = Some(Ustr::from(cpu));
    cgroup_set = true;
  }
  if let Some(pids) = first_value(svc, "TasksMax") {
    cgroup.pids_max = Some(Ustr::from(pids));
    cgroup_set = true;
  }
  if cgroup_set {
    meta.cgroup = Some(cgroup);
  }

  if !after.is_empty() {
    meta.after = Some(after.iter().map(|s| Ustr::from(s)).collect());
  }
  if !start_on.is_empty() {
    meta.start_on = Some(start_on.iter().map(|s| facet_item(s)).collect());
  }

  meta
}

fn build_socket_meta(
  name: &str,
  sock: &HashMap<String, Vec<String>>,
  _after: &[String],
  start_on: &[String],
) -> SocketMetadata {
  let mut meta = SocketMetadata::default();
  meta.name = Ustr::from(name);

  let (kind, listen) = socket_listen(sock);
  meta.r#type = kind.unwrap_or(SocketType::Tcp);
  meta.listen = listen.unwrap_or_else(|| "/".to_string());

  if !start_on.is_empty() {
    meta.start_on = Some(start_on.iter().map(|s| facet_item(s)).collect());
  }

  meta
}

fn socket_listen(sock: &HashMap<String, Vec<String>>) -> (Option<SocketType>, Option<String>) {
  if let Some(v) = first_value(sock, "ListenStream") {
    return (Some(SocketType::Tcp), Some(v.to_string()));
  }
  if let Some(v) = first_value(sock, "ListenDatagram") {
    return (Some(SocketType::Udp), Some(v.to_string()));
  }
  if let Some(v) = first_value(sock, "ListenSequentialPacket") {
    return (Some(SocketType::Tcp), Some(v.to_string()));
  }
  if let Some(v) = first_value(sock, "ListenUNIXSocket") {
    return (Some(SocketType::Uds), Some(v.to_string()));
  }
  if let Some(v) = first_value(sock, "ListenUNIXGRAM") {
    return (Some(SocketType::Udp), Some(v.to_string()));
  }
  if let Some(v) = first_value(sock, "ListenFIFO") {
    return (None, Some(v.to_string()));
  }
  (None, None)
}

fn build_timer_meta(
  name: &str,
  tmr: &HashMap<String, Vec<String>>,
  after: &[String],
) -> Option<TimerMetadata> {
  let duration = timer_duration_from(tmr)?;
  let mut meta = TimerMetadata::default();
  meta.name = Ustr::from(name);
  meta.duration = Ustr::from(&duration);
  if !after.is_empty() {
    meta.after = Some(after.iter().map(|s| Ustr::from(s)).collect());
  }
  Some(meta)
}

fn build_mount_meta(
  name: &str,
  mnt: &HashMap<String, Vec<String>>,
  after: &[String],
) -> MountMetadata {
  let mut meta = MountMetadata::default();

  let target = first_value(mnt, "Where")
    .or_else(|| first_value(mnt, "Path"))
    .unwrap_or(name);
  meta.target = Ustr::from(target);

  if let Some(what) = first_value(mnt, "What") {
    meta.source = Some(Ustr::from(what));
  }
  if let Some(fstype) = first_value(mnt, "Type") {
    meta.fstype = Some(Ustr::from(fstype));
  }
  if let Some(opts) = first_value(mnt, "Options") {
    meta.flags = Some(opts.split(',').map(|s| s.trim().to_string()).collect());
  }
  meta.create = Some(true);

  if !after.is_empty() {
    meta.after = Some(after.iter().map(|s| Ustr::from(s)).collect());
  }

  meta
}

fn collect_env(svc: &HashMap<String, Vec<String>>) -> HashMap<String, String> {
  let mut env: HashMap<String, String> = HashMap::new();
  for v in all_values(svc, "Environment") {
    env.extend(systemd_env_value(&v));
  }
  env
}

fn facet_item(name: &str) -> FlowItem {
  FlowItem::Detailed {
    facet: Some(Ustr::from(name)),
    impulse: None,
    target: None,
    branch: None,
  }
}

fn systemd_restart_to_policy(value: &str) -> Option<RestartPolicy> {
  match value.trim() {
    "no" | "" => None,
    "always" => Some(RestartPolicy::Bool(true)),
    "on-success" | "on-failure" | "on-abnormal" | "on-abort" | "on-watchdog" => {
      Some(RestartPolicy::OnFailure {
        max_retries: u32::MAX,
      })
    }
    _ => None,
  }
}

fn systemd_loader(
  metadata: &mut Metadata,
  content: &str,
  group: Ustr,
  _path: &Path,
  _ctx: &mut OrchestratorContext<'_>,
) -> CoreResult<Void> {
  let ini = parse_ini(content);
  let name = group.as_str();
  load_into(name, &ini, metadata);
  Ok(Void)
}

fn register_systemd_loaders(_name: &str, reg: &mut RegisterLoader) -> CoreResult<Void> {
  reg.register("service", Box::new(systemd_loader));
  reg.register("socket", Box::new(systemd_loader));
  reg.register("timer", Box::new(systemd_loader));
  reg.register("mount", Box::new(systemd_loader));
  reg.register("automount", Box::new(systemd_loader));
  reg.register("target", Box::new(systemd_loader));
  Ok(Void)
}

plugin!(
  name: "systemd-loader",
  version: 1,
  caps: PluginCapability::EXTENSIONS,
  deps: &["units"],
  create: SystemdLoaderPlugin,
  orchestrators: [],
  extensions: [act(register_systemd_loaders)],
  struct SystemdLoaderPlugin;
);

plugin_abi!(1);

#[cfg(test)]
mod tests {
  use super::*;
  use rind_primitives::prelude::Variable;

  fn build_metadata() -> Metadata {
    Metadata::new("static")
      .of::<Service>("service")
      .of::<Socket>("socket")
      .of::<Timer>("timer")
      .of::<Mount>("mount")
      .of::<Variable>("variable")
      .of::<FlowFacet>("facet")
  }

  #[test]
  fn parse_ini_extracts_sections_and_keys() {
    let src = "\
[Unit]
Description=Foo
After=bar.target
Requires=baz.service

[Service]
Type=simple
ExecStart=/usr/bin/foo --flag
Restart=on-failure
";
    let ini = parse_ini(src);
    let unit = ini
      .sections
      .iter()
      .find(|(n, _)| n == "Unit")
      .expect("unit section");
    assert_eq!(first_value(&unit.1, "Description"), Some("Foo"));
    assert_eq!(first_value(&unit.1, "After"), Some("bar.target"));
    let svc = ini
      .sections
      .iter()
      .find(|(n, _)| n == "Service")
      .expect("service section");
    assert_eq!(
      first_value(&svc.1, "ExecStart"),
      Some("/usr/bin/foo --flag")
    );
    assert_eq!(first_value(&svc.1, "Restart"), Some("on-failure"));
  }

  #[test]
  fn parse_ini_handles_continuations_and_comments() {
    let src = "\
# top
[Unit] ; inline comment
Description = A \\
            multi-line \\
            value
";
    let ini = parse_ini(src);
    let unit = ini.sections.iter().find(|(n, _)| n == "Unit").unwrap();
    let desc = first_value(&unit.1, "Description").unwrap();
    assert!(desc.contains("multi-line"), "got: {desc}");
  }

  #[test]
  fn systemd_exec_splits_simple_command() {
    let (exec, args) = systemd_exec("/usr/bin/foo --bar baz").unwrap();
    assert_eq!(exec, "/usr/bin/foo");
    assert_eq!(args, vec!["--bar", "baz"]);
  }

  #[test]
  fn systemd_exec_handles_quoted_args() {
    let (exec, args) = systemd_exec(r#"/usr/bin/foo --name "hello world" --flag"#).unwrap();
    assert_eq!(exec, "/usr/bin/foo");
    assert_eq!(args, vec!["--name", "hello world", "--flag"]);
  }

  #[test]
  fn systemd_restart_to_policy_maps_values() {
    assert_eq!(
      systemd_restart_to_policy("always"),
      Some(RestartPolicy::Bool(true))
    );
    assert_eq!(
      systemd_restart_to_policy("on-failure"),
      Some(RestartPolicy::OnFailure {
        max_retries: u32::MAX
      })
    );
    assert_eq!(systemd_restart_to_policy("no"), None);
  }

  #[test]
  fn parse_time_spec_supports_units() {
    assert_eq!(parse_time_spec("5s"), Some("5s".to_string()));
    assert_eq!(parse_time_spec("3m"), Some("3m".to_string()));
    assert_eq!(parse_time_spec("2h"), Some("2h".to_string()));
    assert_eq!(parse_time_spec("1d"), Some("1d".to_string()));
    assert_eq!(parse_time_spec("2w"), Some("14d".to_string()));
  }

  #[test]
  fn parse_on_calendar_supports_keywords() {
    assert_eq!(parse_on_calendar("daily"), Some("1d".to_string()));
    assert_eq!(parse_on_calendar("weekly"), Some("7d".to_string()));
    assert_eq!(parse_on_calendar("hourly"), Some("1h".to_string()));
    assert_eq!(parse_on_calendar("yearly"), Some("365d".to_string()));
  }

  #[test]
  fn load_socket_inserts_metadata() {
    let mut m = build_metadata();
    let src = "\
[Socket]
ListenStream=0.0.0.0:8080
";
    let ini = parse_ini(src);
    load_into("web", &ini, &mut m);
    let sock = m.get_in_group::<Socket>("web").unwrap();
    assert_eq!(sock.len(), 1);
    assert_eq!(sock[0].name.as_str(), "web");
    assert_eq!(sock[0].listen, "0.0.0.0:8080");
  }

  #[test]
  fn load_socket_uds_path() {
    let mut m = build_metadata();
    let src = "\
[Socket]
ListenUNIXSocket=/run/foo.sock
";
    let ini = parse_ini(src);
    load_into("foo", &ini, &mut m);
    let sock = m.get_in_group::<Socket>("foo").unwrap();
    assert_eq!(sock[0].listen, "/run/foo.sock");
  }

  #[test]
  fn load_timer_inserts_metadata() {
    let mut m = build_metadata();
    let src = "\
[Timer]
OnCalendar=daily
";
    let ini = parse_ini(src);
    load_into("daily-rotate", &ini, &mut m);
    let tmr = m.get_in_group::<Timer>("daily-rotate").unwrap();
    assert_eq!(tmr.len(), 1);
    assert_eq!(tmr[0].name.as_str(), "daily-rotate");
    assert_eq!(tmr[0].duration.as_str(), "1d");
  }

  #[test]
  fn load_timer_with_onbootsec() {
    let mut m = build_metadata();
    let src = "\
[Timer]
OnBootSec=30s
";
    let ini = parse_ini(src);
    load_into("boot-timer", &ini, &mut m);
    let tmr = m.get_in_group::<Timer>("boot-timer").unwrap();
    assert_eq!(tmr[0].duration.as_str(), "30s");
  }

  #[test]
  fn load_mount_inserts_metadata() {
    let mut m = build_metadata();
    let src = "\
[Mount]
What=/dev/sda1
Where=/mnt/data
Type=ext4
Options=defaults,noatime
";
    let ini = parse_ini(src);
    load_into("mnt-data", &ini, &mut m);
    let mounts = m.get_in_group::<Mount>("mnt-data").unwrap();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].target.as_str(), "/mnt/data");
    assert_eq!(
      mounts[0]
        .source
        .as_ref()
        .map(|s: &rind_core::types::Ustr| s.as_str()),
      Some("/dev/sda1")
    );
    assert_eq!(
      mounts[0]
        .fstype
        .as_ref()
        .map(|s: &rind_core::types::Ustr| s.as_str()),
      Some("ext4")
    );
    assert_eq!(mounts[0].create, Some(true));
  }

  #[test]
  fn load_service_inserts_metadata() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=hello
After=network-online.target
WantedBy=multi-user.target

[Service]
Type=simple
ExecStart=/usr/bin/hello --greet=hi
Restart=on-failure
User=makano
WorkingDirectory=/srv/hello
Environment=FOO=bar BAZ=qux
";
    let ini = parse_ini(src);
    load_into("hello", &ini, &mut m);

    let svc = m.get_in_group::<Service>("hello").unwrap();
    assert_eq!(svc.len(), 1);
    assert_eq!(svc[0].name.as_str(), "hello");
    let run = svc[0].run.as_one();
    assert_eq!(run.exec.as_str(), "/usr/bin/hello");
    assert_eq!(
      run.args.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
      vec!["--greet=hi"]
    );
    let env = run.env.as_ref().expect("env should be set");
    assert_eq!(
      env
        .get(&"FOO".to_ustr())
        .map(|v: &rind_core::types::Ustr| v.as_str()),
      Some("bar")
    );
    assert_eq!(
      env
        .get(&"BAZ".to_ustr())
        .map(|v: &rind_core::types::Ustr| v.as_str()),
      Some("qux")
    );
    assert_eq!(
      svc[0]
        .working_dir
        .as_ref()
        .map(|s: &rind_core::types::Ustr| s.as_str()),
      Some("/srv/hello")
    );
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(facets.contains(&"net:online!".to_string()), "{facets:?}");
    assert!(facets.contains(&"rind:up!".to_string()), "{facets:?}");
  }

  #[test]
  fn target_alias_maps_known_targets() {
    assert_eq!(target_to_facet("network-online.target"), "net:online!");
    assert_eq!(target_to_facet("network.target"), "net:configured!");
    assert_eq!(target_to_facet("network-pre.target"), "net:configured!");
    assert_eq!(target_to_facet("multi-user.target"), "rind:up!");
  }

  #[test]
  fn target_alias_strips_unknown_suffix() {
    assert_eq!(target_to_facet("weird.target"), "systemd:weird");
  }

  #[test]
  fn service_with_known_target_puts_targets_in_start_on() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=needs the net
WantedBy=network-online.target
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/needs-net
";
    let ini = parse_ini(src);
    load_into("needs-net", &ini, &mut m);

    let svc = m.get_in_group::<Service>("needs-net").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(
      facets.contains(&"net:online!".to_string()),
      "facets: {facets:?}"
    );
    assert!(
      facets.contains(&"net:configured!".to_string()),
      "facets: {facets:?}"
    );

    let facet_names = collect_facet_names(&m, "needs-net");
    assert!(
      facet_names.is_empty(),
      "service should not emit facets: {facet_names:?}"
    );
  }

  #[test]
  fn service_with_unknown_target_puts_targets_in_start_on() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=joins multi-user
WantedBy=multi-user.target

[Service]
Type=simple
ExecStart=/usr/bin/joins-mu
";
    let ini = parse_ini(src);
    load_into("joins-mu", &ini, &mut m);

    let svc = m.get_in_group::<Service>("joins-mu").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert_eq!(facets, vec!["rind:up!"]);

    let facet_names = collect_facet_names(&m, "joins-mu");
    assert!(
      facet_names.is_empty(),
      "service should not emit facets: {facet_names:?}"
    );
  }

  #[test]
  fn target_file_yields_facet_only() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=Multi-User System
Requires=basic.target
";
    let ini = parse_ini(src);
    load_into("multi-user", &ini, &mut m);

    let names = collect_facet_names(&m, "multi-user");
    assert!(names.contains(&"rind:up!".to_string()), "{names:?}");
    assert!(names.contains(&"rind:basic".to_string()), "{names:?}");

    let services = m.get_in_group::<Service>("multi-user");
    assert!(services.is_none() || services.unwrap().is_empty());
  }

  fn collect_facet_names(m: &Metadata, group: &str) -> Vec<String> {
    let Some(items) = m.get_in_group::<FlowFacet>(group.to_ustr()) else {
      return Vec::new();
    };
    items.iter().map(|f| f.name.to_string()).collect()
  }

  fn facet_name_of(item: &rind_flow::FlowItem) -> String {
    match item {
      rind_flow::FlowItem::Simple(u) => u.to_string(),
      rind_flow::FlowItem::Detailed { facet, .. } => {
        facet.as_ref().map(|u| u.to_string()).unwrap_or_default()
      }
    }
  }

  #[test]
  fn target_ref_known_target() {
    assert_eq!(
      target_ref("network-online.target").as_deref(),
      Some("net:online!")
    );
    assert_eq!(target_ref("multi-user.target").as_deref(), Some("rind:up!"));
    assert_eq!(
      target_ref("network.target").as_deref(),
      Some("net:configured!")
    );
  }

  #[test]
  fn target_ref_unknown_target() {
    assert_eq!(
      target_ref("custom.target").as_deref(),
      Some("systemd:custom")
    );
  }

  #[test]
  fn target_ref_non_target_string() {
    assert!(target_ref("foo.service").is_none());
    assert!(target_ref("bar.socket").is_none());
    assert!(target_ref("plain").is_none());
    assert!(target_ref("").is_none());
  }

  #[test]
  fn target_ref_whitespace_trimmed() {
    assert_eq!(
      target_ref("  network-online.target  ").as_deref(),
      Some("net:online!")
    );
  }

  #[test]
  fn referenced_targets_from_after_and_wants() {
    let mut unit = HashMap::new();
    unit.insert(
      "After".to_string(),
      vec!["network.target".to_string(), "local-fs.target".to_string()],
    );
    unit.insert(
      "Wants".to_string(),
      vec!["network-online.target".to_string()],
    );
    let result = referenced_targets_from(Some(&unit), &[]);
    assert!(
      result.contains(&"net:configured!".to_string()),
      "{result:?}"
    );
    assert!(result.contains(&"net:online!".to_string()), "{result:?}");
    assert!(
      result.contains(&"systemd:local-fs".to_string()),
      "{result:?}"
    );
  }

  #[test]
  fn referenced_targets_from_wanted_by() {
    let mut unit = HashMap::new();
    unit.insert(
      "WantedBy".to_string(),
      vec!["multi-user.target".to_string()],
    );
    let result = referenced_targets_from(Some(&unit), &[]);
    assert_eq!(result, vec!["rind:up!"]);
  }

  #[test]
  fn referenced_targets_from_required_by() {
    let mut unit = HashMap::new();
    unit.insert(
      "RequiredBy".to_string(),
      vec!["network-online.target".to_string()],
    );
    let result = referenced_targets_from(Some(&unit), &[]);
    assert_eq!(result, vec!["net:online!"]);
  }

  #[test]
  fn referenced_targets_from_requires() {
    let mut unit = HashMap::new();
    unit.insert("Requires".to_string(), vec!["basic.target".to_string()]);
    let result = referenced_targets_from(Some(&unit), &[]);
    assert_eq!(result, vec!["rind:basic"]);
  }

  #[test]
  fn referenced_targets_from_wanted_list() {
    let result =
      referenced_targets_from(None, &["net:online!".to_string(), "rind:up!".to_string()]);
    assert_eq!(result, vec!["net:online!", "rind:up!"]);
  }

  #[test]
  fn referenced_targets_from_merges_wanted_and_unit() {
    let mut unit = HashMap::new();
    unit.insert("After".to_string(), vec!["network.target".to_string()]);
    let result = referenced_targets_from(Some(&unit), &["rind:up!".to_string()]);
    assert!(
      result.contains(&"net:configured!".to_string()),
      "{result:?}"
    );
    assert!(result.contains(&"rind:up!".to_string()), "{result:?}");
  }

  #[test]
  fn referenced_targets_from_skips_non_target_tokens() {
    let mut unit = HashMap::new();
    unit.insert(
      "After".to_string(),
      vec![
        "network-online.target".to_string(),
        "some.service".to_string(),
      ],
    );
    let result = referenced_targets_from(Some(&unit), &[]);
    assert!(result.contains(&"net:online!".to_string()), "{result:?}");
    assert!(!result.iter().any(|s| s.contains("some")), "{result:?}");
  }

  #[test]
  fn referenced_targets_from_deduplicates() {
    let mut unit = HashMap::new();
    unit.insert("After".to_string(), vec!["network.target".to_string()]);
    unit.insert("Wants".to_string(), vec!["network.target".to_string()]);
    let result = referenced_targets_from(Some(&unit), &[]);
    assert_eq!(result, vec!["net:configured!"]);
  }

  #[test]
  fn referenced_targets_from_empty_unit() {
    let result = referenced_targets_from(Some(&HashMap::new()), &[]);
    assert!(result.is_empty());
  }

  #[test]
  fn target_file_with_multiple_dependencies_emits_all_facets() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=Basic System
Requires=sysinit.target
Wants=sockets.target
After=local-fs.target
";
    let ini = parse_ini(src);
    load_into("basic", &ini, &mut m);

    let names = collect_facet_names(&m, "basic");
    assert!(names.contains(&"rind:basic".to_string()), "{names:?}");
    assert!(names.contains(&"rind:sockets".to_string()), "{names:?}");
    assert!(names.contains(&"systemd:local-fs".to_string()), "{names:?}");

    let services = m.get_in_group::<Service>("basic");
    assert!(services.is_none() || services.unwrap().is_empty());
  }

  #[test]
  fn target_file_no_unit_section_emits_own_facet() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=Empty target
";
    let ini = parse_ini(src);
    load_into("empty", &ini, &mut m);

    let names = collect_facet_names(&m, "empty");
    assert!(names.contains(&"systemd:empty".to_string()), "{names:?}");
    assert_eq!(names.len(), 1, "{names:?}");
  }

  #[test]
  fn service_with_after_only_puts_facets_in_start_on() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=after net only
After=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/after-net
";
    let ini = parse_ini(src);
    load_into("after-net", &ini, &mut m);

    let svc = m.get_in_group::<Service>("after-net").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(facets.contains(&"net:online!".to_string()), "{facets:?}");

    let facet_names = collect_facet_names(&m, "after-net");
    assert!(
      facet_names.is_empty(),
      "service should not emit facets: {facet_names:?}"
    );
  }

  #[test]
  fn service_with_requires_puts_facets_in_start_on() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=requires basic
Requires=basic.target

[Service]
Type=simple
ExecStart=/usr/bin/requires-basic
";
    let ini = parse_ini(src);
    load_into("requires-basic", &ini, &mut m);

    let svc = m.get_in_group::<Service>("requires-basic").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(facets.contains(&"rind:basic".to_string()), "{facets:?}");

    let facet_names = collect_facet_names(&m, "requires-basic");
    assert!(
      facet_names.is_empty(),
      "service should not emit facets: {facet_names:?}"
    );
  }

  #[test]
  fn service_with_no_targets_has_no_start_on() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=standalone service

[Service]
Type=simple
ExecStart=/usr/bin/standalone
";
    let ini = parse_ini(src);
    load_into("standalone", &ini, &mut m);

    let svc = m.get_in_group::<Service>("standalone").unwrap();
    assert!(
      svc[0].start_on.is_none(),
      "start_on should be None for no targets"
    );
  }

  #[test]
  fn service_with_non_target_after_goes_to_after_list() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=needs another service
After=other.service network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/mixed
";
    let ini = parse_ini(src);
    load_into("mixed", &ini, &mut m);

    let svc = m.get_in_group::<Service>("mixed").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(
      facets.contains(&"net:online!".to_string()),
      "target should be in start_on: {facets:?}"
    );
  }

  #[test]
  fn service_wanted_by_multiple_targets() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=multi target service
WantedBy=network-online.target multi-user.target

[Service]
Type=simple
ExecStart=/usr/bin/multi
";
    let ini = parse_ini(src);
    load_into("multi", &ini, &mut m);

    let svc = m.get_in_group::<Service>("multi").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(facets.contains(&"net:online!".to_string()), "{facets:?}");
    assert!(facets.contains(&"rind:up!".to_string()), "{facets:?}");

    let facet_names = collect_facet_names(&m, "multi");
    assert!(
      facet_names.is_empty(),
      "service should not emit facets: {facet_names:?}"
    );
  }

  #[test]
  fn service_after_and_wants_same_target_deduplicates() {
    let mut m = build_metadata();
    let src = "\
[Unit]
Description=double ref
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/double
";
    let ini = parse_ini(src);
    load_into("double", &ini, &mut m);

    let svc = m.get_in_group::<Service>("double").unwrap();
    let start_on = svc[0].start_on.as_ref().expect("start-on");
    let facets: Vec<String> = start_on.iter().map(facet_name_of).collect();
    assert!(facets.contains(&"net:online!".to_string()), "{facets:?}");
  }
}
