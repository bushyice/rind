use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::Ustr;

pub fn read_env_file(path: &str) -> HashMap<String, String> {
  let mut out = HashMap::new();
  let Ok(content) = std::fs::read_to_string(path) else {
    return out;
  };

  for raw in content.lines() {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }

    // include|source path/to/file.env
    if line.starts_with("include") || line.starts_with("source") {
      if let Some((_, v)) = line.split_once(' ') {
        out.extend(read_env_file(
          PathBuf::from(path)
            .parent()
            .unwrap()
            .join(v)
            .to_str()
            .unwrap_or(v),
        ));
      }
      continue;
    }

    if let Some((k, v)) = line.split_once('=') {
      out.insert(k.trim().to_string(), v.trim().to_string());
    }
  }

  out
}

pub fn normalize_uaddr(addr: impl Into<Ustr>, prefix: &str) -> Ustr {
  let addr = addr.into();
  if addr.starts_with(prefix) {
    Ustr::from(addr.strip_prefix(prefix).unwrap_or(""))
  } else {
    addr.clone()
  }
}
