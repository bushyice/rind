use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use nix::sys::socket::{
  AddressFamily, Backlog, SockFlag, SockType, SockaddrIn, SockaddrIn6, UnixAddr, bind, listen,
  setsockopt, socket, sockopt,
};
use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::flow::FlowRuntimePayload;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
  Tcp,
  Udp,
  Uds,
}

#[model(meta_name = name, meta_fields(name, listen, r#type, owner, signal), derive_metadata(Debug, Clone))]
pub struct Socket {
  pub name: Ustr,
  pub listen: String,
  pub r#type: SocketType,
  pub owner: Option<Ustr>,
  pub signal: Option<Ustr>,
  pub fd: RawFd,
  pub active: bool,
}
#[derive(Debug)]
struct SocketRuntimeEntry {
  socket: Ustr,
  socket_name: Ustr,
  owner: Option<Ustr>,
  #[allow(dead_code)]
  fd: std::os::fd::OwnedFd,
}

#[derive(Default)]
pub struct SocketRuntime {
  instances: HashMap<RawFd, SocketRuntimeEntry>,
  paused: HashMap<Ustr, Vec<RawFd>>,
}

impl SocketRuntime {
  fn create_socket(&self, meta: &SocketMetadata) -> anyhow::Result<std::os::fd::OwnedFd> {
    let fd = match meta.r#type {
      SocketType::Tcp => {
        let addr: std::net::SocketAddr = meta.listen.parse()?;
        let family = match addr {
          std::net::SocketAddr::V4(_) => AddressFamily::Inet,
          std::net::SocketAddr::V6(_) => AddressFamily::Inet6,
        };

        let fd = socket(
          family,
          SockType::Stream,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;
        setsockopt(&fd, sockopt::ReuseAddr, &true)?;

        match addr {
          std::net::SocketAddr::V4(a) => bind(fd.as_raw_fd(), &SockaddrIn::from(a))?,
          std::net::SocketAddr::V6(a) => bind(fd.as_raw_fd(), &SockaddrIn6::from(a))?,
        };

        listen(&fd, Backlog::new(128)?)?;
        fd
      }
      SocketType::Uds => {
        let path = PathBuf::from("/var/sock").join(&meta.listen);
        if let Some(p) = path.parent() {
          std::fs::create_dir_all(p)?;
        }
        if path.exists() {
          let _ = std::fs::remove_file(&path);
        }

        let fd = socket(
          AddressFamily::Unix,
          SockType::Stream,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;
        let sockaddr = UnixAddr::new(&path)?;
        bind(fd.as_raw_fd(), &sockaddr)?;
        listen(&fd, Backlog::new(128)?)?;

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
        fd
      }
      SocketType::Udp => {
        let addr: std::net::SocketAddr = meta.listen.parse()?;
        let family = match addr {
          std::net::SocketAddr::V4(_) => AddressFamily::Inet,
          std::net::SocketAddr::V6(_) => AddressFamily::Inet6,
        };
        let fd = socket(
          family,
          SockType::Datagram,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;

        match addr {
          std::net::SocketAddr::V4(a) => bind(fd.as_raw_fd(), &SockaddrIn::from(a))?,
          std::net::SocketAddr::V6(a) => bind(fd.as_raw_fd(), &SockaddrIn6::from(a))?,
        };
        fd
      }
    };
    Ok(fd)
  }

  fn normalize_owner(group: &Ustr, owner: Ustr) -> Ustr {
    if owner.as_str().contains('@') {
      owner
    } else {
      Ustr::from(format!("{group}@{owner}"))
    }
  }

  fn owner_fds(&self, owner: &Ustr) -> Vec<(RawFd, Ustr)> {
    let mut fds: Vec<(RawFd, Ustr)> = self
      .instances
      .iter()
      .filter_map(|(fd, entry)| {
        if entry.owner.as_ref() == Some(owner) {
          Some((*fd, entry.socket_name.clone()))
        } else {
          None
        }
      })
      .collect();
    fds.sort_by_key(|(fd, _)| *fd);
    fds
  }
}

impl Runtime for SocketRuntime {
  fn id(&self) -> &str {
    "sockets"
  }

  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "setup_all" => {
        let sockets = ctx
          .registry
          .metadata
          .items::<Socket>("units")
          .unwrap_or_default();
        for (group, meta) in sockets {
          let full_name = format!("{}@{}", group, meta.name);
          println!("{full_name}");
          if self
            .instances
            .values()
            .any(|entry| entry.socket.as_str() == full_name)
          {
            continue;
          }

          let owner = meta
            .owner
            .clone()
            .map(|owner| Self::normalize_owner(&group, owner));

          let owned_fd = self
            .create_socket(&meta)
            .map_err(|e| CoreError::Custom(format!("failed to create socket {full_name}: {e}")))?;
          let fd = owned_fd.as_raw_fd();

          let _ = ctx
            .registry
            .instantiate_one("units", full_name.clone(), |metadata| {
              Ok(Socket {
                metadata,
                fd,
                active: true,
              })
            })?;

          ctx.resources.action(fd, ("sockets", "drain_incoming"));

          self.instances.insert(
            fd,
            SocketRuntimeEntry {
              socket: full_name.to_ustr(),
              socket_name: meta.name.clone(),
              owner,
              fd: owned_fd,
            },
          );
        }
      }
      "get_all_fds" => {
        let fds: Vec<_> = self.instances.keys().map(|i| *i).collect();
        return Ok(Some(rpayload!({
          "fds": fds
        })));
      }
      "get_inherited_fds" => {
        let owner = payload.get::<Ustr>("owner")?;
        let fds = self.owner_fds(&owner);
        return Ok(Some(rpayload!({
          "fds": fds
        })));
      }
      "resume_fd" => {
        let fd = payload.get::<i32>("fd")? as RawFd;
        ctx.resources.register_resource(fd);
      }
      "resume_fds" => {
        let owner = payload.get::<Ustr>("name")?;
        if let Some(fds) = self.paused.get(&owner) {
          for fd in fds {
            println!("resuming sockets");
            ctx.resources.register_resource(*fd);
          }
        }
      }
      "drain_incoming" => {
        let fd = payload.get::<i32>("fd")? as RawFd;
        let entry = self.instances.get(&fd).ok_or(CoreError::InvalidState(
          "Socket for fd was not found".into(),
        ))?;

        if let Some(owner) = entry.owner.clone() {
          ctx.resources.pause(fd);
          self.paused.entry(owner.clone()).or_default().push(fd);

          let owner_fds = self.owner_fds(&owner);
          let socket_fds: Vec<i32> = owner_fds.iter().map(|(fd, _)| *fd).collect();
          let socket_fd_names: Vec<Ustr> = owner_fds.into_iter().map(|(_, name)| name).collect();

          let _ = dispatch.dispatch(
            "services",
            "start",
            rpayload!({
              "name": owner,
              "socket_fds": socket_fds,
              "socket_fd_names": socket_fd_names,
            }),
          );
        }

        let socket_full_name = entry.socket.clone();

        if let Ok(sock) = ctx.registry.as_one::<Socket>("units", socket_full_name) {
          if let Some(signal_name) = &sock.metadata.signal {
            let _ = dispatch.dispatch(
              "flow",
              "emit_signal",
              FlowRuntimePayload::new(signal_name)
                .payload(
                  serde_json::json!({ "socket": sock.metadata.name, "fd": sock.fd
                  }),
                )
                .into(),
            );
          }
        }
      }
      _ => {}
    }
    Ok(None)
  }
}
