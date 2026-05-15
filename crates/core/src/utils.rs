use libc::{SO_PEERCRED, SOL_SOCKET, getsockopt, ucred};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::{collections::HashMap, os::fd::AsRawFd};

use crate::rslvns;
use crate::types::Ustr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedName {
  pub parent: Ustr,
  pub child: Ustr,
  pub scope: Ustr,
}

pub fn parse_scoped_name(path: &str) -> ScopedName {
  let (parent, child, scope) = rslvns!(res path);
  ScopedName {
    parent: Ustr::from(parent),
    child: Ustr::from(child),
    scope: Ustr::from(scope),
  }
}

pub fn compose_scoped_name(parent: &str, child: &str, scope: Option<&str>) -> String {
  match scope {
    Some(scope) if !scope.is_empty() && scope != "static" => format!("{parent}:{child}@{scope}"),
    _ => format!("{parent}:{child}"),
  }
}

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

pub fn get_peer_cred_stream(stream: &UnixStream) -> std::io::Result<ucred> {
  get_peer_cred(stream.as_raw_fd())
}

pub fn get_peer_cred(fd: i32) -> std::io::Result<ucred> {
  let mut cred: ucred = unsafe { std::mem::zeroed() };
  let mut len = std::mem::size_of::<ucred>() as libc::socklen_t;

  let ret = unsafe {
    getsockopt(
      fd,
      SOL_SOCKET,
      SO_PEERCRED,
      &mut cred as *mut _ as *mut _,
      &mut len,
    )
  };

  if ret == -1 {
    return Err(std::io::Error::last_os_error());
  }

  Ok(cred)
}

/// Resolve identifier namespaces such as `rind:active`
/// ## Examples
/// ```
/// use rind_core::rslvns;
/// use rind_core::types::ToUstr;
///
/// let _ = rslvns!("rind", "active"); // "rind:active"
/// let _ = rslvns!("units", "rind", "active"); // "units:rind:active"
/// let _ = rslvns!(u "rind", "active"); // ustr of "rind:active"
/// let _ = rslvns!(u "units", "rind", "active"); // ustr of "units:rind:active"
/// let _ = rslvns!(norm "rind:active@static"); // "rind:active"
/// let _ = rslvns!(norm "@scope" "rind:active@scope"); // "rind:active"
/// let _ = rslvns!(res "rind:active"); // ("rind", "active", "static")
/// let _ = rslvns!(res "units:rind:active@s"); // ("rind", "active", "s")
/// ```
#[macro_export]
macro_rules! rslvns {
  ($p:expr, $c:expr) => {
    format!("{}:{}", $p, $c)
  };
  ($u:expr, $p:expr, $c:expr) => {
    format!("{}:{}:{}", $u, $p, $c)
  };

  (u $p:expr, $c:expr) => {
    rslvns!($p, $c).to_ustr()
  };
  (u $u:expr, $p:expr, $c:expr) => {
    rslvns!($u, $p, $c).to_ustr()
  };

  (scp $p:expr, $c:expr) => {
    format!("{}@{}", $c, $p)
  };

  (norm $path:expr) => {{ rslvns!(norm "@static" $path) }};
  (norm $prefix:literal $path:expr) => {{ $path.trim_end_matches($prefix) }};

  (res $path:expr) => {{
    let mut scoped_parts = $path.splitn(2, '@');
    let name = scoped_parts.next().unwrap_or("");
    let scope = scoped_parts.next().unwrap_or("static");
    let mut parts = name.rsplitn(3, ':');
    let child = parts.next().unwrap_or("");
    let parent = parts.next().unwrap_or("");
    (parent, child, scope)
  }};
}

#[cfg(test)]
#[test]
pub fn resolve_namespace() {
  assert_eq!(rslvns!("a", "b"), "a:b");
  assert_eq!(rslvns!("u", "a", "b"), "u:a:b");
  assert_eq!(rslvns!(norm "a:b@static"), "a:b");
  assert_eq!(rslvns!(norm "@s" "a:b@s"), "a:b");
  assert_eq!(rslvns!(res "a:b"), ("a", "b", "static"));
  assert_eq!(rslvns!(res "a:b@s"), ("a", "b", "s"));
}
