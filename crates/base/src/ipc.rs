use libc::{SO_PEERCRED, SOL_SOCKET, getsockopt, ucred};
use rind_ipc::Run0AuthPayload;
use rind_ipc::ser::StateSerialized;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::flow::{State, StateMachineShared};
use crate::mount::{Mount, is_mounted};
use crate::permissions::{PERM_LOGIN, PERM_NETWORK, PERM_RUN0, PERM_SYSTEM_SERVICES};
use crate::services::Service;
use rind_core::prelude::*;
use rind_ipc::{
  LoginPayload, LogoutPayload, Message, MessagePayload, MessageType, NetworkOp, NetworkPayload,
  UnitType,
  ser::{
    MountSerialized, NetworkStatusSerialized, PortStateSerialized, ServiceSerialized,
    UnitItemsSerialized, UnitSerialized, serialize_many,
  },
};

pub const IPC_RUNTIME_ID: &str = "ipc";

type IpcRequest = (Message, Sender<Message>);

#[derive(Debug, Default)]
enum Run0State {
  #[default]
  Inactive,
  RequireAuth,
}

pub struct IpcRuntime {
  incoming_tx: Sender<IpcRequest>,
  incoming_rx: Arc<Mutex<Receiver<IpcRequest>>>,
  listener_thread: Option<thread::JoinHandle<()>>,
  run0_queue: Arc<Mutex<HashMap<i32, Run0State>>>,
}

impl Default for IpcRuntime {
  fn default() -> Self {
    let (tx, rx) = mpsc::channel();
    Self {
      incoming_tx: tx,
      incoming_rx: Arc::new(Mutex::new(rx)),
      listener_thread: None,
      run0_queue: Arc::new(Mutex::new(HashMap::new())),
    }
  }
}

impl Runtime for IpcRuntime {
  fn id(&self) -> &str {
    IPC_RUNTIME_ID
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "start_server" => {
        if self.listener_thread.is_none() {
          let tx = self.incoming_tx.clone();
          self.listener_thread = Some(thread::spawn(move || {
            let socket_path = "/tmp/rind.sock";
            let _ = std::fs::remove_file(socket_path);
            let listener = match UnixListener::bind(socket_path) {
              Ok(l) => l,
              Err(e) => {
                // TODO: i'll use log instead
                eprintln!("[ipc] failed to bind {}: {}", socket_path, e);
                return;
              }
            };

            std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666))
              .expect("failed to allow permissions");

            for stream in listener.incoming() {
              if let Ok(stream) = stream {
                let tx = tx.clone();
                thread::spawn(move || {
                  handle_client_connection(stream, tx);
                });
              }
            }
          }));
        }
      }
      "drain_requests" => {
        let queue = self.run0_queue.clone();
        if let Ok(rx) = self.incoming_rx.lock() {
          while let Ok((msg, reply_tx)) = rx.try_recv() {
            let response = handle_ipc_message(queue.clone(), msg, ctx, dispatch, log);
            let _ = reply_tx.send(response);
          }
        }
      }
      _ => {}
    }
    Ok(())
  }
}

pub fn get_peer_cred(stream: &UnixStream) -> std::io::Result<ucred> {
  let fd = stream.as_raw_fd();

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

fn handle_client_connection(mut stream: UnixStream, parent_tx: Sender<IpcRequest>) {
  let cred = get_peer_cred(&stream).expect("failed to get cred");
  loop {
    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
      break;
    }
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    if stream.read_exact(&mut buf).is_err() {
      break;
    }

    let raw = match String::from_utf8(buf) {
      Ok(s) => s,
      Err(_) => continue,
    };

    let msg: Message = match serde_json::from_str::<Message>(&raw) {
      Ok(m) => {
        if cred.uid == 0 && m.from_uid.is_some() {
          m
        } else {
          m.from_gid(cred.gid).from_uid(cred.uid).from_pid(cred.pid)
        }
      }
      Err(_) => continue,
    };

    let (reply_tx, reply_rx) = mpsc::channel::<Message>();
    if parent_tx.send((msg, reply_tx)).is_err() {
      break;
    }

    let response: Message = match reply_rx.recv() {
      Ok(resp) => resp,
      Err(_) => break,
    };

    let resp_str = response.as_string().into_bytes();
    let resp_len = (resp_str.len() as u32).to_be_bytes();

    if stream.write_all(&resp_len).is_err() {
      break;
    }
    if stream.write_all(&resp_str).is_err() {
      break;
    }
  }
}

fn handle_ipc_message(
  queue: Arc<Mutex<HashMap<i32, Run0State>>>,
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Message {
  let pm = ctx
    .scope
    .get::<PermissionStore>()
    .cloned()
    .unwrap_or_default();

  let sm_shared = ctx
    .scope
    .get::<StateMachineShared>()
    .cloned()
    .unwrap_or_default();

  match msg.r#type {
    MessageType::List => {
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::from_type(MessageType::Error)
          .with(format!("Incorrect Payload: {:?}", msg.payload));
      };

      if payload.unit_type == UnitType::Unit {
        let services = if let Some(services) = ctx
          .registry
          .metadata
          .group_items::<Service>("units", &payload.name)
        {
          services
        } else {
          Vec::new()
        };
        let mounts = if let Some(mounts) = ctx
          .registry
          .metadata
          .group_items::<Mount>("units", &payload.name)
        {
          mounts
        } else {
          Vec::new()
        };

        let ser_instances: HashMap<&String, (String, Vec<u32>)> = services
          .iter()
          .filter_map(|ser| {
            ctx
              .registry
              .as_one::<Service>("units", &format!("{}@{}", payload.name, ser.name))
              .map_or(None, |x| {
                Some((&ser.name, (x.instances.last_state(), x.instances.pid())))
              })
          })
          .collect();

        Message::from_type(MessageType::List).with(
          UnitItemsSerialized {
            mounts: mounts
              .iter()
              .map(|mnt| MountSerialized {
                fstype: mnt.fstype.clone(),
                mounted: is_mounted(&mnt.target).unwrap_or(false),
                source: mnt.source.clone(),
                target: mnt.target.clone(),
              })
              .collect(),
            services: services
              .iter()
              .map(|svc| ServiceSerialized {
                after: svc.after.clone(),
                run: svc.run.to_string(),
                last_state: ser_instances
                  .get(&svc.name)
                  .map_or("Inactive".into(), |x| x.0.clone()),
                name: svc.name().to_string(),
                pid: ser_instances
                  .get(&svc.name)
                  .map_or(None, |x| if x.1.is_empty() { None } else { Some(x.1[0]) }),
                restart: svc.restart.as_ref().map_or(false, |_| true),
              })
              .collect(),
          }
          .stringify(),
        )
      } else if payload.unit_type == UnitType::Service {
        let Some(service_meta) = ctx
          .registry
          .metadata
          .find::<Service>("units", &payload.name)
        else {
          return Message::from_type(MessageType::Error)
            .with(format!("Service not found: {}", payload.name));
        };

        let service = if let Ok(s) = ctx.registry.as_one::<Service>("units", &payload.name) {
          s
        } else {
          &Service::new(service_meta)
        };

        Message::from_type(MessageType::List).with(
          ServiceSerialized {
            name: service.metadata.name.clone(),
            after: service.metadata.after.clone(),
            last_state: service.instances.last_state(),
            pid: service.instances.pid().get(0).cloned(),
            restart: service.metadata.restart.as_ref().map_or(false, |_| true),
            run: service.metadata.run.to_string(),
          }
          .stringify(),
        )
      } else if payload.unit_type == UnitType::State && !payload.name.is_empty() {
        let states = &sm_shared.read().unwrap();
        let Some(instances) = states.states.get(&payload.name) else {
          return Message::from_type(MessageType::Error)
            .with(format!("State not found: {:?}", payload.name));
        };
        let Some(def) = ctx.registry.metadata.find::<State>("units", &payload.name) else {
          return Message::from_type(MessageType::Error)
            .with(format!("State not found: {:?}", payload.name));
        };
        let branches = def.branch.as_ref();

        Message::from_type(MessageType::List).with(
          StateSerialized {
            name: payload.name,
            instances: instances.iter().map(|x| x.payload.to_json()).collect(),
            keys: if let Some(branches) = branches {
              branches.clone()
            } else {
              Default::default()
            },
          }
          .stringify(),
        )
      } else if payload.unit_type == UnitType::State {
        let states = &sm_shared.read().unwrap().states;

        Message::from_type(MessageType::List).with(
          serde_json::to_string(
            &states
              .iter()
              .filter_map(|(name, inst)| {
                let def = ctx.registry.metadata.find::<State>("units", name)?;
                let branches = def.branch.as_ref()?;
                Some(StateSerialized {
                  name: name.clone(),
                  instances: inst.iter().map(|x| x.payload.to_json()).collect(),
                  keys: branches.clone(),
                })
              })
              .collect::<Vec<StateSerialized>>(),
          )
          .unwrap_or_default(),
        )
      } else {
        let mut units_map: HashMap<String, UnitSerialized> = HashMap::new();

        if let Some(groups) = ctx.registry.metadata.groups("units") {
          for group in groups {
            let services = if let Some(services) = ctx
              .registry
              .metadata
              .group_items::<Service>("units", &group)
            {
              services
            } else {
              Vec::new()
            };
            let mounts =
              if let Some(mounts) = ctx.registry.metadata.group_items::<Mount>("units", &group) {
                mounts
              } else {
                Vec::new()
              };

            let mounted = mounts
              .iter()
              .filter(|mnt| is_mounted(&mnt.target).unwrap_or(false))
              .count();
            let active = services
              .iter()
              .filter(|s| {
                ctx
                  .registry
                  .instances::<Service>("units", &format!("{group}@{}", s.name))
                  .ok()
                  .map_or(false, |x| x.iter().any(|x| x.instances.is_active()))
              })
              .count();
            units_map.insert(
              group.to_string(),
              UnitSerialized {
                active_services: active,
                mounted: mounted,
                mounts: mounts.len(),
                name: group.to_string(),
                services: services.len(),
              },
            );
          }
        }

        let mut units_list: Vec<UnitSerialized> = units_map.into_values().collect();
        units_list.sort_by(|a, b| a.name.cmp(&b.name));
        Message::from_type(MessageType::List).with(serialize_many(&units_list))
      }
    }
    MessageType::Start => {
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::nack("`start payload");
      };

      if msg.from_uid.is_none() || !pm.user_has(msg.from_uid.unwrap(), PERM_SYSTEM_SERVICES) {
        return Message::nack("Permission Denied");
      }

      let _ = dispatch.dispatch(
        "services",
        "start",
        serde_json::json!({ "name": payload.name }).into(),
      );
      Message::ack(format!("started {}", payload.name))
    }
    MessageType::Stop => {
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::nack("invalid stop payload");
      };

      if msg.from_uid.is_none() || !pm.user_has(msg.from_uid.unwrap(), PERM_SYSTEM_SERVICES) {
        return Message::nack("Permission Denied");
      }

      let force = payload.force.unwrap_or(false);
      let _ = dispatch.dispatch("services", "stop", serde_json::json!({ "name": payload.name, "mode": if force { "force" } else { "graceful" } }).into());
      Message::ack(format!("stopped {}", payload.name))
    }
    MessageType::Run0 => {
      let Some(pid) = msg.from_pid else {
        return Message::nack("Permission Denied");
      };
      let Some(uid) = msg.from_uid else {
        return Message::nack("Permission Denied");
      };

      if !pm.user_has(msg.from_uid.unwrap(), PERM_RUN0)
        && !pm.users.user_in_group(
          pm.users.lookup_by_uid(msg.from_uid.unwrap()).unwrap(),
          "wheel",
        )
      {
        return Message::nack("Permission Denied");
      }

      let mut run0_queue = queue.lock().unwrap();

      let state = run0_queue.entry(pid).or_insert(Run0State::default());

      if matches!(state, Run0State::Inactive) {
        *state = Run0State::RequireAuth;
        return Message::from_type(MessageType::RequestPassword);
      }

      let Some(payload) = msg.parse_payload::<Run0AuthPayload>().ok() else {
        return Message::nack("invalid stop payload");
      };

      let pam = ctx
        .scope
        .get::<Arc<rind_core::user::PamHandle>>()
        .expect("PamHandle not in scope");

      let Some(user) = pam.store().lookup_by_uid(uid) else {
        return Message::nack("user not found");
      };

      let password = payload.password;
      if let Err(e) = pam.pam_authenticate(&user.username, &password) {
        run0_queue.remove(&pid);
        return Message::nack(format!("authentication failed: {e:?}"));
      }

      if matches!(state, Run0State::RequireAuth) {
        // let env = std::fs::read(format!("/proc/{}/environ", pid))
        //   .map(|bytes| {
        //     bytes
        //       .split(|b| *b == 0) // split on '\0'
        //       .filter_map(|entry| {
        //         let s = std::str::from_utf8(entry).ok()?;
        //         s.split_once('=')
        //           .map(|(k, v)| (k.to_string(), v.to_string()))
        //       })
        //       .collect::<HashMap<String, String>>()
        //   })
        //   .unwrap_or_default();

        // let cwd = std::fs::read_link(format!("/proc/{}/cwd", pid)).unwrap_or(PathBuf::from("/"));

        // use std::fs::File;

        // let Ok(stdin) = File::open(format!("/proc/{}/fd/0", pid)) else {
        //   return Message::nack(format!("failed to read stdin for parent process"));
        // };
        // let Ok(stdout) = File::open(format!("/proc/{}/fd/1", pid)) else {
        //   return Message::nack(format!("failed to read stdout for parent process"));
        // };
        // let Ok(stderr) = File::open(format!("/proc/{}/fd/2", pid)) else {
        //   return Message::nack(format!("failed to read stderr for parent process"));
        // };

        // let args = args.clone();
        run0_queue.remove(&pid);

        // std::thread::spawn(move || {
        //   let mut args = args.into_iter();
        //   let program = args.next().unwrap();

        //   let mut command = Command::new(program);

        //   command
        //     .args(args)
        //     .gid(0)
        //     .uid(0)
        //     .envs(env)
        //     .current_dir(cwd)
        //     .stdin(stdin)
        //     .stdout(stdout)
        //     .stderr(stderr);

        //   let _ = command.spawn();
        // });

        Message::from_type(MessageType::Valid)
      } else {
        Message::from_type(MessageType::Unknown)
      }
    }
    MessageType::Login => {
      let Some(payload) = msg.parse_payload::<LoginPayload>().ok() else {
        return Message::nack("invalid login payload");
      };

      if msg.from_uid.is_none() || !pm.user_has(msg.from_uid.unwrap(), PERM_LOGIN) {
        return Message::nack("Permission Denied");
      }

      let pam = ctx
        .scope
        .get::<Arc<rind_core::user::PamHandle>>()
        .expect("PamHandle not in scope");

      let Some(_) = pam.store().lookup_by_name(&payload.username) else {
        return Message::nack("user not found");
      };

      let password = payload.password.as_deref().unwrap_or("");
      if let Err(e) = pam.pam_authenticate(&payload.username, password) {
        return Message::nack(format!("authentication failed: {e:?}"));
      }

      if let Err(e) = pam.pam_acct_mgmt(&payload.username) {
        return Message::nack(format!("account validation failed: {e:?}"));
      }

      let session = match pam.pam_open_session(&payload.username, &payload.tty) {
        Ok(s) => s,
        Err(e) => return Message::nack(format!("session error: {e:?}")),
      };

      let _ = dispatch.dispatch(
        "user",
        "login",
        serde_json::json!({
          "username": payload.username.clone(),
          "tty": payload.tty.clone(),
          "session_id": session.id,
        })
        .into(),
      );

      Message::ack(format!("logged in successfully as {}", payload.username))
    }
    MessageType::Logout => {
      let Some(mut payload) = msg.parse_payload::<LogoutPayload>().ok() else {
        return Message::nack("invalid logout payload");
      };

      if !payload.tty.starts_with("/dev/") {
        payload.tty = format!("/dev/{}", payload.tty);
      }

      let pam = ctx
        .scope
        .get::<Arc<rind_core::user::PamHandle>>()
        .expect("PamHandle not in scope");

      let Some(user) = pam.store().lookup_by_name(&payload.username) else {
        return Message::nack("user not found");
      };

      if msg.from_uid.is_none() || msg.from_uid.unwrap() != user.uid {
        return Message::nack("Permission Denied");
      }

      let sessions = pam.sessions_for(&payload.username);

      let mut closed = false;
      let mut session_id = 0;
      for session in sessions {
        if session.tty == payload.tty {
          session_id = session.id;
          let _ = pam.pam_close_session(session.id);
          closed = true;
        }
      }

      if closed {
        let _ = dispatch.dispatch(
          "user",
          "logout",
          serde_json::json!({
            "session_id": session_id,
            "username": payload.username,
          })
          .into(),
        );

        Message::ack(format!("logged out {}", payload.username))
      } else {
        Message::nack(format!(
          "no active session for {} on tty {}",
          payload.username, payload.tty
        ))
      }
    }
    MessageType::Network => {
      let Some(payload) = msg.parse_payload::<NetworkPayload>().ok() else {
        return Message::nack("invalid network payload");
      };

      match payload.op {
        NetworkOp::Status => {
          let mut statuses = Vec::new();
          let sm = sm_shared.read().unwrap();
          if let Some(groups) = ctx.registry.metadata.groups("units") {
            for group in groups {
              if let Some(cfgs) = ctx
                .registry
                .metadata
                .group_items::<crate::networking::NetworkConfig>("units", &group)
              {
                for cfg in cfgs {
                  let config = {
                    if let Some(instances) = sm.states.get("rind@net-configured") {
                      instances.iter().find(|i| {
                        if let Some(obj) = i.payload.to_json().as_object() {
                          obj.get("name").and_then(|v| v.as_str()) == Some(&cfg.name)
                        } else {
                          false
                        }
                      })
                    } else {
                      None
                    }
                  };
                  let state = if config.is_some() {
                    "Configured"
                  } else {
                    "Down"
                  }
                  .to_string();
                  statuses.push(NetworkStatusSerialized {
                    interface: cfg.name.clone(),
                    method: match cfg.method {
                      crate::networking::NetworkMethod::Dhcp => "dhcp".to_string(),
                      crate::networking::NetworkMethod::Static => "static".to_string(),
                    },
                    address: config
                      .map(|x| x.payload.get_json_field_as::<String>("ip"))
                      .unwrap_or_default(),
                    gateway: config
                      .map(|x| x.payload.get_json_field_as::<String>("gateway"))
                      .unwrap_or_default(),
                    state,
                  });
                }
              }
            }
          }
          Message::from_type(MessageType::List).with_vec(statuses)
        }
        NetworkOp::Ports => {
          #[allow(unused)]
          #[derive(Clone, Copy)]
          enum TcpState {
            Established = 1,
            SynSent,
            SynRecv,
            FinWait1,
            FinWait2,
            TimeWait,
            Close,
            CloseWait,
            LastAck,
            Listen,
            Closing,
            NewSynRecv,
          }

          impl TcpState {
            fn as_str(&self) -> &'static str {
              match self {
                Self::Established => "ESTABLISHED",
                Self::SynSent => "SYN_SENT",
                Self::SynRecv => "SYN_RECV",
                Self::FinWait1 => "FIN_WAIT1",
                Self::FinWait2 => "FIN_WAIT2",
                Self::TimeWait => "TIME_WAIT",
                Self::Close => "CLOSE",
                Self::CloseWait => "CLOSE_WAIT",
                Self::LastAck => "LAST_ACK",
                Self::Listen => "LISTEN",
                Self::Closing => "CLOSING",
                Self::NewSynRecv => "NEW_SYN_RECV",
              }
            }
          }

          let mut ports = Vec::new();

          let parse_ip_port = |s: &str| -> Option<(String, u16)> {
            let mut parts = s.split(':');
            let ip_hex = parts.next()?;
            let port_hex = parts.next()?;

            let ip_int = u32::from_str_radix(ip_hex, 16).ok()?;
            let port = u16::from_str_radix(port_hex, 16).ok()?;

            let ip = std::net::Ipv4Addr::from(u32::from_be(ip_int));
            Some((ip.to_string(), port))
          };

          let mut inode_to_pid: HashMap<u64, (u32, String)> = HashMap::new();
          if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
              let name = entry.file_name();
              let name_str = name.to_string_lossy();
              if let Ok(pid) = name_str.parse::<u32>() {
                if let Ok(fds) = std::fs::read_dir(format!("/proc/{}/fd", pid)) {
                  let proc_name = std::fs::read_to_string(format!("/proc/{}/comm", pid))
                    .unwrap_or_else(|_| "unknown".to_string())
                    .trim()
                    .to_string();
                  for fd_entry in fds.flatten() {
                    if let Ok(target) = std::fs::read_link(fd_entry.path()) {
                      let t_str = target.to_string_lossy();
                      if t_str.starts_with("socket:[") && t_str.ends_with("]") {
                        if let Ok(inode) = t_str[8..t_str.len() - 1].parse::<u64>() {
                          inode_to_pid.insert(inode, (pid, proc_name.clone()));
                        }
                      }
                    }
                  }
                }
              }
            }
          }

          if let Ok(content) = std::fs::read_to_string("/proc/net/tcp") {
            for line in content.lines().skip(1) {
              let parts: Vec<&str> = line.split_whitespace().collect();
              if parts.len() < 4 {
                continue;
              }

              if let Some((ip, port)) = parse_ip_port(parts[1]) {
                let state_hex = parts[3];
                let state_int = u8::from_str_radix(state_hex, 16).unwrap_or(0);

                let state_str = match state_int {
                  1 => TcpState::Established.as_str(),
                  10 => TcpState::Listen.as_str(),
                  _ => "UNKNOWN",
                };

                let inode = parts.get(9).and_then(|s| s.parse::<u64>().ok());
                let (pid, process) = inode
                  .and_then(|i| inode_to_pid.get(&i))
                  .map(|(p, n)| (Some(*p), Some(n.clone())))
                  .unwrap_or((None, None));

                ports.push(PortStateSerialized {
                  protocol: "tcp".to_string(),
                  local_address: ip,
                  local_port: port,
                  state: state_str.to_string(),
                  pid,
                  process,
                });
              }
            }
          }

          if let Ok(content) = std::fs::read_to_string("/proc/net/udp") {
            for line in content.lines().skip(1) {
              let parts: Vec<&str> = line.split_whitespace().collect();
              if parts.len() < 10 {
                continue;
              }

              if let Some((ip, port)) = parse_ip_port(parts[1]) {
                let inode = parts.get(9).and_then(|s| s.parse::<u64>().ok());
                let (pid, process) = inode
                  .and_then(|i| inode_to_pid.get(&i))
                  .map(|(p, n)| (Some(*p), Some(n.clone())))
                  .unwrap_or((None, None));

                ports.push(PortStateSerialized {
                  protocol: "udp".to_string(),
                  local_address: ip,
                  local_port: port,
                  state: "UNCONN".to_string(),
                  pid,
                  process,
                });
              }
            }
          }

          Message::from_type(MessageType::List).with_vec(ports)
        }
        NetworkOp::Set {
          iface,
          method,
          address: _,
          gateway: _,
        } => {
          if msg.from_uid.is_none() || !pm.user_has(msg.from_uid.unwrap(), PERM_NETWORK) {
            return Message::nack("Permission Denied for network modifications");
          }

          if method == "up" || method == "down" {
            unsafe {
              let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
              if sock >= 0 {
                let mut req: libc::ifreq = std::mem::zeroed();
                let bytes = iface.as_bytes();
                let len = std::cmp::min(bytes.len(), 15);
                std::ptr::copy_nonoverlapping(
                  bytes.as_ptr(),
                  req.ifr_name.as_mut_ptr() as *mut u8,
                  len,
                );
                if libc::ioctl(sock, libc::SIOCGIFFLAGS, &mut req) == 0 {
                  if method == "up" {
                    req.ifr_ifru.ifru_flags |= libc::IFF_UP as i16 | libc::IFF_RUNNING as i16;
                  } else if method == "down" {
                    req.ifr_ifru.ifru_flags &= !(libc::IFF_UP as i16);
                  }
                  libc::ioctl(sock, libc::SIOCSIFFLAGS, &req);
                }
                libc::close(sock);
              }
            }
            Message::ack(format!("Interface {} set {}", iface, method))
          } else {
            Message::ack("network configuration applied (mock output)")
          }
        }
      }
    }
    _ => Message::from_type(MessageType::Unknown),
  }
}
