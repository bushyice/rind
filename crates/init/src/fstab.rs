use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct FstabEntry {
  pub device: String,
  pub mountpoint: String,
  pub fstype: String,
  pub options: Vec<String>,
  #[allow(unused)]
  pub dump: u32,
  #[allow(unused)]
  pub pass: u32,
}

// impl FstabEntry {
//   pub fn is_mount_in_fstab(target: &str, fstab_path: &str) -> bool {
//     crate::fstab::parse_file(fstab_path)
//       .ok()
//       .map(|entries: Vec<FstabEntry>| entries.iter().any(|e| e.mountpoint == target))
//       .unwrap_or(false)
//   }
// }

pub fn parse_file(path: impl AsRef<Path>) -> Result<Vec<FstabEntry>, String> {
  let file = File::open(path.as_ref())
    .map_err(|e| format!("failed to open fstab '{}': {e}", path.as_ref().display()))?;
  let reader = BufReader::new(file);
  let mut entries = Vec::new();

  for (line_num, line) in reader.lines().enumerate() {
    let line = line.map_err(|e| format!("failed to read fstab line: {e}"))?;
    let line = line.trim();

    if line.is_empty() || line.starts_with('#') {
      continue;
    }

    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 4 {
      eprintln!(
        "[fstab] skipping malformed line {} (expected 4-6 fields, got {})",
        line_num + 1,
        fields.len()
      );
      continue;
    }

    let device = fields[0].to_string();
    let mountpoint = fields[1].to_string();
    let fstype = fields[2].to_string();
    let options: Vec<String> = fields[3].split(',').map(String::from).collect();
    let dump = fields.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let pass = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);

    entries.push(FstabEntry {
      device,
      mountpoint,
      fstype,
      options,
      dump,
      pass,
    });
  }

  Ok(entries)
}

pub fn parse() -> Result<Vec<FstabEntry>, String> {
  parse_file("/etc/fstab")
}
