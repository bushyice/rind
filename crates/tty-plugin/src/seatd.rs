use std::{
  collections::HashMap,
  io::{Read, Write},
  os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
  os::unix::fs::PermissionsExt,
  os::unix::net::{UnixListener, UnixStream},
  path::PathBuf,
  sync::atomic::{AtomicI32, Ordering},
  thread,
};

use rind_core::reexports::libc;

const CLIENT_OPEN_SEAT: u16 = 1;
const CLIENT_CLOSE_SEAT: u16 = 2;
const CLIENT_OPEN_DEVICE: u16 = 3;
const CLIENT_CLOSE_DEVICE: u16 = 4;
const CLIENT_DISABLE_SEAT: u16 = 5;
const CLIENT_SWITCH_SESSION: u16 = 6;
const CLIENT_PING: u16 = 7;

const fn server_op(op: u16) -> u16 {
  (1 << 15) | op
}

const SERVER_SEAT_OPENED: u16 = server_op(1);
const SERVER_SEAT_CLOSED: u16 = server_op(2);
const SERVER_DEVICE_OPENED: u16 = server_op(3);
const SERVER_DEVICE_CLOSED: u16 = server_op(4);
const SERVER_ENABLE_SEAT: u16 = server_op(6);
const SERVER_PONG: u16 = server_op(7);
const SERVER_SESSION_SWITCHED: u16 = server_op(8);
const SERVER_SEAT_DISABLED: u16 = server_op(9);
const SERVER_ERROR: u16 = server_op(0x7FFF);

#[repr(C, packed)]
struct ProtoHeader {
  opcode: u16,
  size: u16,
}

#[repr(C, packed)]
struct ProtoServerSeatOpened {
  seat_name_len: u16,
}

#[repr(C, packed)]
struct ProtoClientOpenDevice {
  path_len: u16,
}

#[repr(C, packed)]
struct ProtoServerDeviceOpened {
  device_id: i32,
}

#[repr(C, packed)]
struct ProtoClientCloseDevice {
  device_id: i32,
}

#[repr(C, packed)]
struct ProtoClientSwitchSession {
  session: i32,
}

#[repr(C, packed)]
struct ProtoServerError {
  error_code: i32,
}

fn send_raw(stream: &mut UnixStream, buf: &[u8]) -> Result<(), i32> {
  let mut off = 0;
  while off < buf.len() {
    match stream.write(&buf[off..]) {
      Ok(0) => return Err(libc::EPIPE),
      Ok(n) => off += n,
      Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
      Err(e) => return Err(e.raw_os_error().unwrap_or(libc::EIO)),
    }
  }
  Ok(())
}

fn send_header(stream: &mut UnixStream, opcode: u16, size: u16) -> Result<(), i32> {
  let h = ProtoHeader { opcode, size };
  send_raw(stream, unsafe {
    std::slice::from_raw_parts(
      &h as *const _ as *const u8,
      std::mem::size_of::<ProtoHeader>(),
    )
  })
}

fn send_response(stream: &mut UnixStream, opcode: u16, payload: &[u8]) -> Result<(), i32> {
  send_header(stream, opcode, payload.len() as u16)?;
  if !payload.is_empty() {
    send_raw(stream, payload)?;
  }
  Ok(())
}

fn send_msg_with_fd(
  stream: &mut UnixStream,
  opcode: u16,
  payload: &[u8],
  fd: RawFd,
) -> Result<(), i32> {
  let header = ProtoHeader {
    opcode,
    size: payload.len() as u16,
  };
  unsafe {
    let cmsg_space = libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) as usize;
    let mut cmsg_buf = vec![0u8; cmsg_space];

    let iov = &mut [
      libc::iovec {
        iov_base: (&header as *const ProtoHeader) as *mut libc::c_void,
        iov_len: std::mem::size_of::<ProtoHeader>(),
      },
      libc::iovec {
        iov_base: payload.as_ptr() as *mut libc::c_void,
        iov_len: payload.len(),
      },
    ];

    let mut msg: libc::msghdr = std::mem::zeroed();
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = 2;

    let cmsg = &mut *(cmsg_buf.as_mut_ptr() as *mut libc::cmsghdr);
    cmsg.cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as libc::size_t;
    cmsg.cmsg_level = libc::SOL_SOCKET;
    cmsg.cmsg_type = libc::SCM_RIGHTS;
    *(libc::CMSG_DATA(cmsg) as *mut libc::c_int) = fd;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len();

    if libc::sendmsg(stream.as_raw_fd(), &msg, 0) == -1 {
      return Err(
        std::io::Error::last_os_error()
          .raw_os_error()
          .unwrap_or(libc::EIO),
      );
    }
  }
  Ok(())
}

fn send_error(stream: &mut UnixStream, err: i32) -> Result<(), i32> {
  let msg = ProtoServerError { error_code: err };
  send_response(stream, SERVER_ERROR, unsafe {
    std::slice::from_raw_parts(
      &msg as *const ProtoServerError as *const u8,
      std::mem::size_of::<ProtoServerError>(),
    )
  })
}

const VT_ACTIVATE: libc::c_ulong = 0x5606;

fn vt_switch_to(session: i32) -> Result<(), i32> {
  let file = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open("/dev/tty0")
    .map_err(|_| libc::ENOENT)?;
  let fd = file.as_raw_fd();
  unsafe {
    if libc::ioctl(fd, VT_ACTIVATE, session as libc::c_ulong) == -1 {
      return Err(
        std::io::Error::last_os_error()
          .raw_os_error()
          .unwrap_or(libc::EIO),
      );
    }
  }
  Ok(())
}

#[allow(dead_code)]
fn get_current_vt() -> Option<i32> {
  let content = std::fs::read_to_string("/sys/class/tty/tty0/active").ok()?;
  let name = content.trim();
  let n = name.strip_prefix("tty")?.parse::<i32>().ok()?;
  Some(n)
}

fn realpath(path: &str) -> Option<String> {
  unsafe {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let ptr = libc::realpath(
      path.as_ptr() as *const libc::c_char,
      buf.as_mut_ptr() as *mut libc::c_char,
    );
    if ptr.is_null() {
      return None;
    }
    let len = libc::strnlen(ptr, libc::PATH_MAX as usize);
    Some(std::str::from_utf8(&buf[..len]).ok()?.to_string())
  }
}

fn handle_client(mut stream: UnixStream) {
  let peer = stream.peer_addr().ok();
  println!("[seatd] client connected: {:?}", peer);

  if let Err(e) = run_client(&mut stream) {
    eprintln!("[seatd] client error: errno={}", e);
  }

  eprintln!("[seatd] client disconnected: {:?}", peer);
}

fn read_exact(stream: &mut UnixStream, buf: &mut [u8]) -> Result<(), i32> {
  let mut off = 0;
  while off < buf.len() {
    match stream.read(&mut buf[off..]) {
      Ok(0) => return Err(libc::EPIPE),
      Ok(n) => off += n,
      Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
      Err(e) => return Err(e.raw_os_error().unwrap_or(libc::EIO)),
    }
  }
  Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum ClientState {
  New,
  Active,
  Disabled,
  Closed,
}

struct Device {
  _path: String,
  _fd: OwnedFd,
}

fn run_client(stream: &mut UnixStream) -> Result<(), i32> {
  let mut client_state = ClientState::New;
  let mut devices: HashMap<i32, Device> = HashMap::new();
  let next_device_id = AtomicI32::new(1);
  let mut has_seat = false;

  loop {
    let mut header_buf = [0u8; std::mem::size_of::<ProtoHeader>()];
    read_exact(stream, &mut header_buf)?;
    let header: ProtoHeader = unsafe { std::mem::transmute::<[u8; 4], ProtoHeader>(header_buf) };

    let size = header.size as usize;
    let payload = if size > 0 {
      let mut buf = vec![0u8; size];
      read_exact(stream, &mut buf)?;
      buf
    } else {
      vec![]
    };

    match header.opcode {
      CLIENT_OPEN_SEAT => {
        if has_seat {
          return send_error(stream, libc::EBUSY);
        }
        has_seat = true;

        let seat_name = "seat0\0";
        let seat_name_len = seat_name.len() as u16;
        let rmsg = ProtoServerSeatOpened { seat_name_len };
        let rmsg_bytes = unsafe {
          std::slice::from_raw_parts(
            &rmsg as *const _ as *const u8,
            std::mem::size_of::<ProtoServerSeatOpened>(),
          )
        };

        let header_out = ProtoHeader {
          opcode: SERVER_SEAT_OPENED,
          size: (std::mem::size_of::<ProtoServerSeatOpened>() + seat_name.len()) as u16,
        };
        send_raw(stream, unsafe {
          std::slice::from_raw_parts(
            &header_out as *const _ as *const u8,
            std::mem::size_of::<ProtoHeader>(),
          )
        })?;
        send_raw(stream, rmsg_bytes)?;
        send_raw(stream, seat_name.as_bytes())?;

        send_response(stream, SERVER_ENABLE_SEAT, &[])?;
        client_state = ClientState::Active;
      }

      CLIENT_CLOSE_SEAT => {
        if !has_seat {
          return send_error(stream, libc::ENOTCONN);
        }
        devices.clear();
        has_seat = false;
        client_state = ClientState::Closed;
        send_response(stream, SERVER_SEAT_CLOSED, &[])?;
      }

      CLIENT_OPEN_DEVICE => {
        if !has_seat || client_state != ClientState::Active {
          return send_error(stream, libc::EPERM);
        }
        if payload.len() < std::mem::size_of::<ProtoClientOpenDevice>() {
          return send_error(stream, libc::EINVAL);
        }
        let path_len =
          unsafe { (payload.as_ptr() as *const ProtoClientOpenDevice).read_unaligned() }.path_len
            as usize;
        if payload.len() < std::mem::size_of::<ProtoClientOpenDevice>() + path_len {
          return send_error(stream, libc::EINVAL);
        }
        let path_bytes = &payload[std::mem::size_of::<ProtoClientOpenDevice>()..][..path_len];
        let path = std::str::from_utf8(path_bytes).map_err(|_| libc::EINVAL)?;
        let path = path.trim_end_matches('\0');

        let canonical = realpath(path).ok_or(libc::ENOENT)?;

        let file = std::fs::OpenOptions::new()
          .read(true)
          .write(true)
          .open(&canonical)
          .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?;
        let fd = file.as_raw_fd();
        let dup_fd = unsafe { libc::dup(fd) };
        if dup_fd == -1 {
          return Err(
            std::io::Error::last_os_error()
              .raw_os_error()
              .unwrap_or(libc::EIO),
          );
        }
        let device_id = next_device_id.fetch_add(1, Ordering::SeqCst);
        let device = Device {
          _path: canonical.clone(),
          _fd: unsafe { OwnedFd::from_raw_fd(dup_fd) },
        };
        devices.insert(device_id, device);

        let rmsg = ProtoServerDeviceOpened { device_id };
        let rmsg_bytes = unsafe {
          std::slice::from_raw_parts(
            &rmsg as *const _ as *const u8,
            std::mem::size_of::<ProtoServerDeviceOpened>(),
          )
        };
        send_msg_with_fd(stream, SERVER_DEVICE_OPENED, rmsg_bytes, fd)?;
      }

      CLIENT_CLOSE_DEVICE => {
        if !has_seat || client_state != ClientState::Active {
          return send_error(stream, libc::EPERM);
        }
        if payload.len() < std::mem::size_of::<ProtoClientCloseDevice>() {
          return send_error(stream, libc::EINVAL);
        }
        let device_id =
          unsafe { (payload.as_ptr() as *const ProtoClientCloseDevice).read_unaligned() }.device_id;
        if devices.remove(&device_id).is_none() {
          return send_error(stream, libc::EBADF);
        }
        send_response(stream, SERVER_DEVICE_CLOSED, &[])?;
      }

      CLIENT_SWITCH_SESSION => {
        if !has_seat {
          return send_error(stream, libc::ENOTCONN);
        }
        if payload.len() < std::mem::size_of::<ProtoClientSwitchSession>() {
          return send_error(stream, libc::EINVAL);
        }
        let session =
          unsafe { (payload.as_ptr() as *const ProtoClientSwitchSession).read_unaligned() }.session;
        if session <= 0 {
          return send_error(stream, libc::EINVAL);
        }
        vt_switch_to(session)?;
        send_response(stream, SERVER_SESSION_SWITCHED, &[])?;
      }

      CLIENT_DISABLE_SEAT => {
        if !has_seat || client_state != ClientState::Active {
          return send_error(stream, libc::EPERM);
        }
        client_state = ClientState::Disabled;
        send_response(stream, SERVER_SEAT_DISABLED, &[])?;
      }

      CLIENT_PING => {
        send_response(stream, SERVER_PONG, &[])?;
      }

      _ => {
        let op = header.opcode;
        eprintln!("[seatd] unknown opcode: {op}");
        send_error(stream, libc::ENOSYS)?;
      }
    }
  }
}

pub fn start() {
  let socket_path = std::env::var("SEATD_SOCK")
    .map(PathBuf::from)
    .unwrap_or_else(|_| PathBuf::from("/run/seatd.sock"));

  let _ = std::fs::remove_file(&socket_path);

  let listener = match UnixListener::bind(&socket_path) {
    Ok(l) => l,
    Err(e) => {
      eprintln!("[seatd] failed to bind {socket_path:?}: {e}");
      return;
    }
  };

  let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o666));
  println!("[seatd] listening on {socket_path:?}");

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        thread::spawn(|| handle_client(stream));
      }
      Err(e) => {
        eprintln!("[seatd] accept error: {e}");
        break;
      }
    }
  }

  eprintln!("[seatd] server stopped");
}
