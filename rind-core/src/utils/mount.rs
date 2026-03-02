use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub fn is_mounted(target: impl Into<PathBuf>) -> std::io::Result<bool> {
  let file = File::open("/proc/self/mountinfo")?;
  let reader = BufReader::new(file);
  let target = &target.into();

  for line in reader.lines() {
    let line = line?;
    // mountinfo format: see man 5 proc
    // fields: ID, parent, major:minor, root, mount_point, ...
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() > 4 && parts[4] == target.to_string_lossy() {
      return Ok(true);
    }
  }
  Ok(false)
}
