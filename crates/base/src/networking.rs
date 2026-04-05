use std::collections::HashMap;

use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rind_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

const NETWORKING_INTERFACE_STATE: &str = "rind@net-interface";
const NETWORKING_ONLINE_STATE: &str = "rind@online";
const NETWORKING_CONFIGURED_STATE: &str = "rind@net-configured";
const NETWORKING_DNS_READY_STATE: &str = "rind@net-dns_ready";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMethod {
  Dhcp,
  Static,
}

impl Default for NetworkMethod {
  fn default() -> Self {
    Self::Dhcp
  }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NetworkRoute {
  pub destination: String, // CIDR e.g., "10.0.0.0/8" or "0.0.0.0/0"
  pub gateway: Option<String>,
  pub metric: Option<i32>,
}

#[model(
  meta_name = name,
  meta_fields(name, method, address, gateway, dns, route),
  derive_metadata(Debug, Clone)
)]
pub struct NetworkConfig {
  pub name: String,
  #[serde(default)]
  pub method: NetworkMethod,
  pub address: Option<String>,
  pub gateway: Option<String>,
  pub dns: Option<Vec<String>>,
  pub route: Option<Vec<NetworkRoute>>,

  // runtime
  pub configured: bool,
}

impl NetworkConfig {
  pub fn new(metadata: std::sync::Arc<NetworkConfigMetadata>) -> Self {
    Self {
      metadata,
      configured: false,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WiredInterfaceState {
  name: String,
  kind: String,
  operstate: String,
  carrier: bool,
  mtu: Option<u32>,
  mac: Option<String>,
}

impl WiredInterfaceState {
  fn is_online(&self) -> bool {
    self.carrier || self.operstate == "up"
  }
}

#[derive(Debug, Clone)]
struct DhcpLease {
  ip: Ipv4Addr,
  subnet_mask: Ipv4Addr,
  gateway: Option<Ipv4Addr>,
  dns_servers: Vec<Ipv4Addr>,
  lease_time: u32,
  obtained: Instant,
}

impl DhcpLease {
  fn needs_renewal(&self) -> bool {
    self.obtained.elapsed() > Duration::from_secs((self.lease_time / 2) as u64)
  }

  #[allow(dead_code)]
  fn expired(&self) -> bool {
    self.obtained.elapsed() > Duration::from_secs(self.lease_time as u64)
  }

  fn prefix_len(&self) -> u8 {
    let mask = u32::from(self.subnet_mask);
    mask.count_ones() as u8
  }
}

#[derive(Debug, Clone)]
struct InterfaceRuntimeState {
  ip: Option<Ipv4Addr>,
  gateway: Option<Ipv4Addr>,
  dns_servers: Vec<Ipv4Addr>,
  dhcp_lease: Option<DhcpLease>,
  last_configure_attempt: Option<Instant>,
}

impl Default for InterfaceRuntimeState {
  fn default() -> Self {
    Self {
      ip: None,
      gateway: None,
      dns_servers: Vec::new(),
      dhcp_lease: None,
      last_configure_attempt: None,
    }
  }
}

#[derive(Debug)]
pub struct NetworkingRuntime {
  // scans
  last_interfaces: HashMap<String, WiredInterfaceState>,
  online: bool,
  sys_class_net: PathBuf,
  last_scan: Option<Instant>,
  scan_interval: Duration,

  // config
  interface_states: HashMap<String, InterfaceRuntimeState>,
  dns_written: bool,
  configured_interfaces: Vec<String>,
  bootstrapped: bool,
}

impl Default for NetworkingRuntime {
  fn default() -> Self {
    Self {
      last_interfaces: HashMap::new(),
      online: false,
      sys_class_net: PathBuf::from("/sys/class/net"),
      last_scan: None,
      scan_interval: Duration::from_secs(1),

      interface_states: HashMap::new(),
      dns_written: false,
      configured_interfaces: Vec::new(),
      bootstrapped: false,
    }
  }
}

impl NetworkingRuntime {
  fn scan_and_sync(&mut self, dispatch: &RuntimeDispatcher) -> Result<(), CoreError> {
    if let Some(last_scan) = self.last_scan
      && last_scan.elapsed() < self.scan_interval
    {
      return Ok(());
    }
    self.last_scan = Some(Instant::now());
    let interfaces = collect_wired_interfaces(self.sys_class_net.as_path());

    for (name, interface) in interfaces.iter() {
      if self.last_interfaces.get(name) == Some(interface) {
        continue;
      }

      dispatch.dispatch(
        "flow",
        "set_state",
        json!({
          "name": NETWORKING_INTERFACE_STATE,
          "payload": serde_json::to_value(interface).unwrap_or_default(),
        })
        .into(),
      )?;
    }

    for name in self.last_interfaces.keys() {
      if interfaces.contains_key(name) {
        continue;
      }

      dispatch.dispatch(
        "flow",
        "remove_state",
        json!({
          "name": NETWORKING_INTERFACE_STATE,
          "filter": { "as": { "name": name } },
        })
        .into(),
      )?;
    }

    let next_online = interfaces.values().any(WiredInterfaceState::is_online);
    if next_online != self.online {
      if next_online {
        dispatch.dispatch(
          "flow",
          "set_state",
          json!({ "name": NETWORKING_ONLINE_STATE }).into(),
        )?;
      } else {
        dispatch.dispatch(
          "flow",
          "remove_state",
          json!({ "name": NETWORKING_ONLINE_STATE }).into(),
        )?;
      }
    }

    self.last_interfaces = interfaces;
    self.online = next_online;
    Ok(())
  }

  fn bootstrap(&mut self, ctx: &mut RuntimeContext<'_>, log: &LogHandle) -> Result<(), CoreError> {
    if self.bootstrapped {
      return Ok(());
    }
    self.bootstrapped = true;

    setup_loopback();
    log.log(
      LogLevel::Info,
      "networking",
      "loopback interface configured",
      HashMap::new(),
    );

    let configs = Self::load_network_configs(ctx);
    for (_unit, cfg) in &configs {
      let iface = &cfg.name;
      let sysfs_path = self.sys_class_net.join(iface);
      if sysfs_path.exists() {
        bring_interface_up(iface);
        let mut fields = HashMap::new();
        fields.insert("interface".to_string(), iface.clone());
        log.log(LogLevel::Info, "networking", "brought interface up", fields);
      } else {
        let mut fields = HashMap::new();
        fields.insert("interface".to_string(), iface.clone());
        log.log(
          LogLevel::Warn,
          "networking",
          "interface not found in sysfs",
          fields,
        );
      }
    }

    Ok(())
  }

  fn configure(
    &mut self,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    let configs = Self::load_network_configs(ctx);

    for (unit, cfg) in configs {
      let iface_name = cfg.name.clone();

      if self.configured_interfaces.contains(&iface_name) {
        continue;
      }

      let sysfs_path = self.sys_class_net.join(&iface_name);
      if !sysfs_path.exists() {
        continue;
      }

      if let Some(state) = self.interface_states.get(&iface_name) {
        if let Some(last) = state.last_configure_attempt {
          if last.elapsed() < Duration::from_secs(5) {
            continue;
          }
        }
      }

      let state = self.interface_states.entry(iface_name.clone()).or_default();
      state.last_configure_attempt = Some(Instant::now());

      let mut fields = HashMap::new();
      fields.insert("interface".to_string(), iface_name.clone());
      fields.insert("unit".to_string(), unit.clone());
      fields.insert("method".to_string(), format!("{:?}", cfg.method));

      log.log(
        LogLevel::Info,
        "networking",
        "configuring interface",
        fields,
      );

      match cfg.method {
        NetworkMethod::Dhcp => {
          self.configure_dhcp(&iface_name, dispatch, log)?;
        }
        NetworkMethod::Static => {
          self.configure_static(&iface_name, &cfg, dispatch, log)?;
        }
      }
    }

    Ok(())
  }

  fn load_network_configs(
    ctx: &mut RuntimeContext<'_>,
  ) -> Vec<(String, std::sync::Arc<NetworkConfigMetadata>)> {
    let Some(m) = ctx.registry.metadata.metadata("units") else {
      return Vec::new();
    };
    let mut out = Vec::new();
    for group in m.groups() {
      if let Some(cfgs) = ctx
        .registry
        .metadata
        .group_items::<NetworkConfig>("units", group)
      {
        for c in cfgs {
          out.push((group.to_string(), c));
        }
      }
    }
    out
  }

  fn configure_dhcp(
    &mut self,
    iface: &str,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    let mac = self
      .last_interfaces
      .get(iface)
      .and_then(|s| s.mac.clone())
      .unwrap_or_default();

    let mac_bytes = parse_mac(&mac).unwrap_or([0u8; 6]);

    match dhcp_request(iface, &mac_bytes) {
      Ok(lease) => {
        let mut fields = HashMap::new();
        fields.insert("interface".to_string(), iface.to_string());
        fields.insert("ip".to_string(), lease.ip.to_string());
        if let Some(gw) = lease.gateway {
          fields.insert("gateway".to_string(), gw.to_string());
        }
        log.log(LogLevel::Info, "networking", "DHCP lease acquired", fields);

        self.apply_lease(iface, &lease, dispatch, log)?;

        let state = self.interface_states.entry(iface.to_string()).or_default();
        state.ip = Some(lease.ip);
        state.gateway = lease.gateway;
        state.dns_servers = lease.dns_servers.clone();
        state.dhcp_lease = Some(lease);
      }
      Err(e) => {
        let mut fields = HashMap::new();
        fields.insert("interface".to_string(), iface.to_string());
        fields.insert("error".to_string(), e.to_string());
        log.log(LogLevel::Error, "networking", "DHCP failed", fields);
      }
    }
    Ok(())
  }

  fn configure_static(
    &mut self,
    iface: &str,
    cfg: &NetworkConfigMetadata,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    let Some(address) = &cfg.address else {
      log.log(
        LogLevel::Error,
        "networking",
        "static config missing address",
        {
          let mut f = HashMap::new();
          f.insert("interface".to_string(), iface.to_string());
          f
        },
      );
      return Ok(());
    };

    let (ip, prefix) = parse_cidr(address)
      .ok_or_else(|| CoreError::InvalidState(format!("invalid CIDR address: {address}")))?;

    let mask = prefix_to_mask(prefix);
    let gateway = cfg
      .gateway
      .as_ref()
      .and_then(|g| g.parse::<Ipv4Addr>().ok());

    bring_interface_up(iface);
    set_interface_addr(iface, ip, mask);
    if let Some(gw) = gateway {
      add_specific_route(Ipv4Addr::UNSPECIFIED, 0, Some(gw), 0);
    }

    if let Some(routes) = cfg.route.as_ref() {
      for route in routes {
        if let Some((dst_ip, dst_prefix)) = parse_cidr(&route.destination) {
          let gw_ip = route
            .gateway
            .as_ref()
            .and_then(|g| g.parse::<Ipv4Addr>().ok());
          let metric = route.metric.unwrap_or(0);
          add_specific_route(dst_ip, dst_prefix, gw_ip, metric);
        } else if route.destination == "default" {
          let gw_ip = route
            .gateway
            .as_ref()
            .and_then(|g| g.parse::<Ipv4Addr>().ok());
          let metric = route.metric.unwrap_or(0);
          add_specific_route(Ipv4Addr::UNSPECIFIED, 0, gw_ip, metric);
        }
      }
    }

    let dns_servers: Vec<Ipv4Addr> = cfg
      .dns
      .as_ref()
      .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
      .unwrap_or_default();

    if !dns_servers.is_empty() {
      write_resolv_conf(&dns_servers);
      self.dns_written = true;
      dispatch.dispatch(
        "flow",
        "set_state",
        json!({ "name": NETWORKING_DNS_READY_STATE }).into(),
      )?;
    }

    dispatch.dispatch(
      "flow",
      "set_state",
      json!({
        "name": NETWORKING_CONFIGURED_STATE,
        "payload": {
          "name": iface,
          "ip": format!("{}/{}", ip, prefix),
          "gateway": gateway.map(|g| g.to_string()).unwrap_or_default(),
        }
      })
      .into(),
    )?;

    let state = self.interface_states.entry(iface.to_string()).or_default();
    state.ip = Some(ip);
    state.gateway = gateway;
    state.dns_servers = dns_servers;

    if !self.configured_interfaces.contains(&iface.to_string()) {
      self.configured_interfaces.push(iface.to_string());
    }

    let mut fields = HashMap::new();
    fields.insert("interface".to_string(), iface.to_string());
    fields.insert("ip".to_string(), format!("{}/{}", ip, prefix));
    log.log(
      LogLevel::Info,
      "networking",
      "static config applied",
      fields,
    );

    Ok(())
  }

  fn apply_lease(
    &mut self,
    iface: &str,
    lease: &DhcpLease,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<(), CoreError> {
    bring_interface_up(iface);
    set_interface_addr(iface, lease.ip, lease.subnet_mask);

    if let Some(gw) = lease.gateway {
      add_specific_route(Ipv4Addr::UNSPECIFIED, 0, Some(gw), 0);
    }

    if !lease.dns_servers.is_empty() {
      write_resolv_conf(&lease.dns_servers);
      self.dns_written = true;
      dispatch.dispatch(
        "flow",
        "set_state",
        json!({ "name": NETWORKING_DNS_READY_STATE }).into(),
      )?;
    }

    dispatch.dispatch(
      "flow",
      "set_state",
      json!({
        "name": NETWORKING_CONFIGURED_STATE,
        "payload": {
          "name": iface,
          "ip": format!("{}/{}", lease.ip, lease.prefix_len()),
          "gateway": lease.gateway.map(|g| g.to_string()).unwrap_or_default(),
        }
      })
      .into(),
    )?;

    if !self.configured_interfaces.contains(&iface.to_string()) {
      self.configured_interfaces.push(iface.to_string());
    }

    Ok(())
  }

  fn reconcile(
    &mut self,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    if self.configured_interfaces.is_empty() && self.online {
      self.configure(ctx, dispatch, log)?;
      return Ok(());
    }

    let ifaces_needing_renewal: Vec<String> = self
      .interface_states
      .iter()
      .filter_map(|(iface, state)| {
        state
          .dhcp_lease
          .as_ref()
          .filter(|l| l.needs_renewal())
          .map(|_| iface.clone())
      })
      .collect();

    for iface in ifaces_needing_renewal {
      let mut fields = HashMap::new();
      fields.insert("interface".to_string(), iface.clone());
      log.log(LogLevel::Info, "networking", "DHCP lease renewal", fields);
      self.configure_dhcp(&iface, dispatch, log)?;
    }

    for iface in self.configured_interfaces.clone() {
      let still_online = self
        .last_interfaces
        .get(&iface)
        .map(|s| s.is_online())
        .unwrap_or(false);

      if !still_online {
        dispatch.dispatch(
          "flow",
          "remove_state",
          json!({
            "name": NETWORKING_CONFIGURED_STATE,
            "filter": { "as": { "name": iface } },
          })
          .into(),
        )?;
        self.configured_interfaces.retain(|i| i != &iface);
        self.interface_states.remove(&iface);
      }
    }

    Ok(())
  }
}

impl Runtime for NetworkingRuntime {
  fn id(&self) -> &str {
    "networking"
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
      "scan" => self.scan_and_sync(dispatch)?,
      "bootstrap" => self.bootstrap(ctx, log)?,
      "configure" => self.configure(ctx, dispatch, log)?,
      "reconcile" => self.reconcile(ctx, dispatch, log)?,
      _ => {}
    }

    Ok(())
  }
}

// low level stuff

fn setup_loopback() {
  bring_interface_up("lo");
  set_interface_addr(
    "lo",
    Ipv4Addr::new(127, 0, 0, 1),
    Ipv4Addr::new(255, 0, 0, 0),
  );
}

fn bring_interface_up(iface: &str) {
  unsafe {
    let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
    if sock < 0 {
      return;
    }

    let mut ifr: libc::ifreq = std::mem::zeroed();
    copy_ifname(&mut ifr, iface);

    if libc::ioctl(sock, libc::SIOCGIFFLAGS as _, &mut ifr) == 0 {
      ifr.ifr_ifru.ifru_flags |= libc::IFF_UP as i16 | libc::IFF_RUNNING as i16;
      libc::ioctl(sock, libc::SIOCSIFFLAGS as _, &ifr);
    }

    libc::close(sock);
  }
}

fn set_interface_addr(iface: &str, ip: Ipv4Addr, mask: Ipv4Addr) {
  unsafe {
    let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
    if sock < 0 {
      return;
    }

    let mut ifr: libc::ifreq = std::mem::zeroed();
    copy_ifname(&mut ifr, iface);
    let addr = sockaddr_in(ip);
    std::ptr::copy_nonoverlapping(
      &addr as *const libc::sockaddr_in as *const u8,
      &mut ifr.ifr_ifru as *mut _ as *mut u8,
      std::mem::size_of::<libc::sockaddr_in>(),
    );
    libc::ioctl(sock, libc::SIOCSIFADDR as _, &ifr);

    let mut ifr_mask: libc::ifreq = std::mem::zeroed();
    copy_ifname(&mut ifr_mask, iface);
    let mask_addr = sockaddr_in(mask);
    std::ptr::copy_nonoverlapping(
      &mask_addr as *const libc::sockaddr_in as *const u8,
      &mut ifr_mask.ifr_ifru as *mut _ as *mut u8,
      std::mem::size_of::<libc::sockaddr_in>(),
    );
    libc::ioctl(sock, libc::SIOCSIFNETMASK as _, &ifr_mask);

    libc::close(sock);
  }
}

fn add_specific_route(dest: Ipv4Addr, prefix: u8, gateway: Option<Ipv4Addr>, metric: i32) {
  unsafe {
    let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
    if sock < 0 {
      return;
    }

    let mut rt: libc::rtentry = std::mem::zeroed();

    let dst = sockaddr_in(dest);
    std::ptr::copy_nonoverlapping(
      &dst as *const libc::sockaddr_in as *const u8,
      &mut rt.rt_dst as *mut libc::sockaddr as *mut u8,
      std::mem::size_of::<libc::sockaddr_in>(),
    );

    if let Some(gw) = gateway {
      let gw_addr = sockaddr_in(gw);
      std::ptr::copy_nonoverlapping(
        &gw_addr as *const libc::sockaddr_in as *const u8,
        &mut rt.rt_gateway as *mut libc::sockaddr as *mut u8,
        std::mem::size_of::<libc::sockaddr_in>(),
      );
      rt.rt_flags |= libc::RTF_GATEWAY as u16;
    }

    let mask_ip = prefix_to_mask(prefix);
    let mask = sockaddr_in(mask_ip);
    std::ptr::copy_nonoverlapping(
      &mask as *const libc::sockaddr_in as *const u8,
      &mut rt.rt_genmask as *mut libc::sockaddr as *mut u8,
      std::mem::size_of::<libc::sockaddr_in>(),
    );

    rt.rt_flags |= libc::RTF_UP as u16;
    rt.rt_metric = metric as i16;

    libc::ioctl(sock, libc::SIOCADDRT as _, &rt);
    libc::close(sock);
  }
}

unsafe fn copy_ifname(ifr: &mut libc::ifreq, name: &str) {
  let bytes = name.as_bytes();
  let len = bytes.len().min(libc::IFNAMSIZ - 1);
  unsafe {
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ifr.ifr_name.as_mut_ptr() as *mut u8, len);
  }
}

fn sockaddr_in(addr: Ipv4Addr) -> libc::sockaddr_in {
  let octets = addr.octets();
  libc::sockaddr_in {
    sin_family: libc::AF_INET as u16,
    sin_port: 0,
    sin_addr: libc::in_addr {
      s_addr: u32::from_ne_bytes(octets),
    },
    sin_zero: [0; 8],
  }
}

fn write_resolv_conf(servers: &[Ipv4Addr]) {
  let content = generate_resolv_conf(servers);
  let _ = std::fs::write("/etc/resolv.conf", content);
}

fn generate_resolv_conf(servers: &[Ipv4Addr]) -> String {
  let mut lines = Vec::new();
  lines.push("# Generated by rind networking".to_string());
  for server in servers {
    lines.push(format!("nameserver {server}"));
  }
  lines.push(String::new());
  lines.join("\n")
}

// DHCP minimal
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

// DHCP message types
const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;

// DHCP option codes
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_END: u8 = 255;

fn dhcp_request(iface: &str, mac: &[u8; 6]) -> Result<DhcpLease, CoreError> {
  let socket = UdpSocket::bind(("0.0.0.0", DHCP_CLIENT_PORT))
    .map_err(|e| CoreError::Custom(format!("DHCP: failed to bind socket: {e}")))?;
  socket
    .set_broadcast(true)
    .map_err(|e| CoreError::Custom(format!("DHCP: failed to set broadcast: {e}")))?;
  socket.set_read_timeout(Some(Duration::from_secs(10))).ok();

  use std::os::unix::io::AsRawFd;
  let fd = socket.as_raw_fd();
  let iface_bytes = iface.as_bytes();
  unsafe {
    libc::setsockopt(
      fd,
      libc::SOL_SOCKET,
      libc::SO_BINDTODEVICE,
      iface_bytes.as_ptr() as *const libc::c_void,
      iface_bytes.len() as libc::socklen_t,
    );
  }

  let xid: u32 = {
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs() as u32;
    now ^ u32::from_ne_bytes([mac[2], mac[3], mac[4], mac[5]])
  };

  let discover = build_dhcp_packet(DHCP_DISCOVER, xid, mac, None, None);
  socket
    .send_to(&discover, ("255.255.255.255", DHCP_SERVER_PORT))
    .map_err(|e| CoreError::Custom(format!("DHCP discover send failed: {e}")))?;

  let mut buf = [0u8; 1500];
  let (len, _) = socket
    .recv_from(&mut buf)
    .map_err(|e| CoreError::Custom(format!("DHCP: no offer received: {e}")))?;
  let offer = parse_dhcp_response(&buf[..len], xid)?;

  if offer.msg_type != DHCP_OFFER {
    return Err(CoreError::Custom("DHCP: expected OFFER".into()));
  }

  let offered_ip = offer.your_ip;
  let server_id = offer.server_id;

  let request = build_dhcp_packet(DHCP_REQUEST, xid, mac, Some(offered_ip), server_id);
  socket
    .send_to(&request, ("255.255.255.255", DHCP_SERVER_PORT))
    .map_err(|e| CoreError::Custom(format!("DHCP request send failed: {e}")))?;

  let (len, _) = socket
    .recv_from(&mut buf)
    .map_err(|e| CoreError::Custom(format!("DHCP: no ack received: {e}")))?;
  let ack = parse_dhcp_response(&buf[..len], xid)?;

  if ack.msg_type != DHCP_ACK {
    return Err(CoreError::Custom("DHCP: expected ACK".into()));
  }

  Ok(DhcpLease {
    ip: ack.your_ip,
    subnet_mask: ack.subnet_mask.unwrap_or(Ipv4Addr::new(255, 255, 255, 0)),
    gateway: ack.router,
    dns_servers: ack.dns_servers,
    lease_time: ack.lease_time.unwrap_or(86400),
    obtained: Instant::now(),
  })
}

fn build_dhcp_packet(
  msg_type: u8,
  xid: u32,
  mac: &[u8; 6],
  requested_ip: Option<Ipv4Addr>,
  server_id: Option<Ipv4Addr>,
) -> Vec<u8> {
  let mut pkt = vec![0u8; 240];

  pkt[0] = 1; // bootreq
  pkt[1] = 1; // eth
  pkt[2] = 6; // HW addr length
  pkt[3] = 0; // hops

  pkt[4..8].copy_from_slice(&xid.to_be_bytes());

  // Secs = 0, Flags = broadcast (0x8000)
  pkt[10] = 0x80;
  pkt[11] = 0x00;

  pkt[28..34].copy_from_slice(mac);

  pkt[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);

  let mut opts = Vec::new();

  opts.extend_from_slice(&[OPT_MSG_TYPE, 1, msg_type]);

  if let Some(ip) = requested_ip {
    opts.push(50); // opt 50 = requested IP
    opts.push(4);
    opts.extend_from_slice(&ip.octets());
  }

  if let Some(sid) = server_id {
    opts.push(OPT_SERVER_ID);
    opts.push(4);
    opts.extend_from_slice(&sid.octets());
  }

  opts.extend_from_slice(&[55, 4, OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME]);

  opts.push(OPT_END);

  pkt.extend_from_slice(&opts);
  pkt
}

struct DhcpResponse {
  msg_type: u8,
  your_ip: Ipv4Addr,
  server_id: Option<Ipv4Addr>,
  subnet_mask: Option<Ipv4Addr>,
  router: Option<Ipv4Addr>,
  dns_servers: Vec<Ipv4Addr>,
  lease_time: Option<u32>,
}

fn parse_dhcp_response(data: &[u8], expected_xid: u32) -> Result<DhcpResponse, CoreError> {
  if data.len() < 240 {
    return Err(CoreError::Custom("DHCP: response too short".into()));
  }

  if data[0] != 2 {
    return Err(CoreError::Custom("DHCP: not a BOOTREPLY".into()));
  }

  let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
  if xid != expected_xid {
    return Err(CoreError::Custom("DHCP: XID mismatch".into()));
  }

  let your_ip = Ipv4Addr::new(data[16], data[17], data[18], data[19]);

  if data[236..240] != DHCP_MAGIC_COOKIE {
    return Err(CoreError::Custom("DHCP: bad magic cookie".into()));
  }

  let mut msg_type = 0u8;
  let mut server_id = None;
  let mut subnet_mask = None;
  let mut router = None;
  let mut dns_servers = Vec::new();
  let mut lease_time = None;

  let mut i = 240;
  while i < data.len() {
    let opt = data[i];
    if opt == OPT_END {
      break;
    }
    if opt == 0 {
      // padding
      i += 1;
      continue;
    }
    if i + 1 >= data.len() {
      break;
    }
    let len = data[i + 1] as usize;
    let val_start = i + 2;
    let val_end = val_start + len;
    if val_end > data.len() {
      break;
    }
    let val = &data[val_start..val_end];

    match opt {
      OPT_MSG_TYPE if len >= 1 => msg_type = val[0],
      OPT_SUBNET_MASK if len >= 4 => {
        subnet_mask = Some(Ipv4Addr::new(val[0], val[1], val[2], val[3]));
      }
      OPT_ROUTER if len >= 4 => {
        router = Some(Ipv4Addr::new(val[0], val[1], val[2], val[3]));
      }
      OPT_DNS => {
        let mut j = 0;
        while j + 3 < len {
          dns_servers.push(Ipv4Addr::new(val[j], val[j + 1], val[j + 2], val[j + 3]));
          j += 4;
        }
      }
      OPT_LEASE_TIME if len >= 4 => {
        lease_time = Some(u32::from_be_bytes([val[0], val[1], val[2], val[3]]));
      }
      OPT_SERVER_ID if len >= 4 => {
        server_id = Some(Ipv4Addr::new(val[0], val[1], val[2], val[3]));
      }
      _ => {}
    }

    i = val_end;
  }

  Ok(DhcpResponse {
    msg_type,
    your_ip,
    server_id,
    subnet_mask,
    router,
    dns_servers,
    lease_time,
  })
}

fn collect_wired_interfaces(root: &Path) -> HashMap<String, WiredInterfaceState> {
  let mut interfaces = HashMap::new();
  let Ok(entries) = std::fs::read_dir(root) else {
    return interfaces;
  };

  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().to_string();
    let path = entry.path();
    if !is_wired_interface(name.as_str(), path.as_path()) {
      continue;
    }

    let operstate = read_trimmed(path.join("operstate")).unwrap_or_else(|| "unknown".into());
    let carrier = read_trimmed(path.join("carrier")).map_or(false, |x| x == "1");
    let mtu = read_trimmed(path.join("mtu")).and_then(|x| x.parse::<u32>().ok());
    let mac = read_trimmed(path.join("address"));

    interfaces.insert(
      name.clone(),
      WiredInterfaceState {
        name,
        kind: "wired".to_string(),
        operstate,
        carrier,
        mtu,
        mac,
      },
    );
  }

  interfaces
}

// will be replaced by interface_type later on.
fn is_wired_interface(name: &str, path: &Path) -> bool {
  if name == "lo" {
    return false;
  }

  if path.join("wireless").exists() {
    return false;
  }

  if let Some(kind) = read_trimmed(path.join("type"))
    && kind != "1"
  {
    return false;
  }

  name.starts_with("en") || name.starts_with("eth") || path.join("device").exists()
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
  std::fs::read_to_string(path)
    .ok()
    .map(|x| x.trim().to_string())
    .filter(|x| !x.is_empty())
}

fn parse_mac(mac: &str) -> Option<[u8; 6]> {
  let parts: Vec<&str> = mac.split(':').collect();
  if parts.len() != 6 {
    return None;
  }
  let mut bytes = [0u8; 6];
  for (i, part) in parts.iter().enumerate() {
    bytes[i] = u8::from_str_radix(part, 16).ok()?;
  }
  Some(bytes)
}

fn parse_cidr(cidr: &str) -> Option<(Ipv4Addr, u8)> {
  let (ip_str, prefix_str) = cidr.split_once('/')?;
  let ip = ip_str.parse::<Ipv4Addr>().ok()?;
  let prefix = prefix_str.parse::<u8>().ok()?;
  if prefix > 32 {
    return None;
  }
  Some((ip, prefix))
}

fn prefix_to_mask(prefix: u8) -> Ipv4Addr {
  if prefix == 0 {
    return Ipv4Addr::UNSPECIFIED;
  }
  let mask: u32 = !0u32 << (32 - prefix);
  Ipv4Addr::from(mask)
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::net::Ipv4Addr;
  use std::path::{Path, PathBuf};
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;

  fn temp_dir(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos();
    let root = std::env::temp_dir().join(format!("rind-networking-{tag}-{stamp}"));
    fs::create_dir_all(&root).expect("failed to create temp dir");
    root
  }

  fn mk_iface(root: &Path, name: &str, files: &[(&str, &str)], wireless: bool, with_device: bool) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).expect("failed to create iface dir");
    if wireless {
      fs::create_dir_all(dir.join("wireless")).expect("failed to create wireless marker");
    }
    if with_device {
      fs::create_dir_all(dir.join("device")).expect("failed to create device marker");
    }
    for (file, content) in files {
      fs::write(dir.join(file), content).expect("failed to write iface file");
    }
  }

  #[test]
  fn collect_wired_interfaces_skips_loopback_and_wireless() {
    let root = temp_dir("scan");

    mk_iface(
      &root,
      "lo",
      &[("type", "1"), ("operstate", "unknown"), ("carrier", "1")],
      false,
      false,
    );
    mk_iface(
      &root,
      "wlan0",
      &[("type", "1"), ("operstate", "up"), ("carrier", "1")],
      true,
      true,
    );
    mk_iface(
      &root,
      "enp0s3",
      &[
        ("type", "1"),
        ("operstate", "up"),
        ("carrier", "1"),
        ("mtu", "1500"),
        ("address", "52:54:00:12:34:56"),
      ],
      false,
      true,
    );

    let interfaces = collect_wired_interfaces(&root);
    assert_eq!(interfaces.len(), 1);
    assert!(interfaces.contains_key("enp0s3"));

    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn collect_wired_interfaces_accepts_non_prefixed_device_names() {
    let root = temp_dir("device-fallback");
    mk_iface(
      &root,
      "usb0",
      &[
        ("type", "1"),
        ("operstate", "up"),
        ("carrier", "1"),
        ("address", "00:11:22:33:44:55"),
      ],
      false,
      true,
    );

    let interfaces = collect_wired_interfaces(&root);
    assert!(interfaces.contains_key("usb0"));

    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn dhcp_discover_packet_structure() {
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let xid = 0xDEADBEEF_u32;
    let pkt = build_dhcp_packet(DHCP_DISCOVER, xid, &mac, None, None);

    // BootP
    assert_eq!(pkt[0], 1);
    // eth
    assert_eq!(pkt[1], 1);
    // HW addr len
    assert_eq!(pkt[2], 6);

    assert_eq!(&pkt[4..8], &xid.to_be_bytes());

    // broadcast flag
    assert_eq!(pkt[10], 0x80);

    assert_eq!(&pkt[28..34], &mac);

    assert_eq!(&pkt[236..240], &DHCP_MAGIC_COOKIE);

    assert_eq!(pkt[240], OPT_MSG_TYPE);
    assert_eq!(pkt[241], 1);
    assert_eq!(pkt[242], DHCP_DISCOVER);
  }

  #[test]
  fn dhcp_request_packet_includes_requested_ip_and_server_id() {
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let xid = 0x12345678;
    let requested_ip = Ipv4Addr::new(192, 168, 1, 100);
    let server_id = Ipv4Addr::new(192, 168, 1, 1);
    let pkt = build_dhcp_packet(DHCP_REQUEST, xid, &mac, Some(requested_ip), Some(server_id));

    // message type = REQUEST
    assert_eq!(pkt[240], OPT_MSG_TYPE);
    assert_eq!(pkt[242], DHCP_REQUEST);

    // requested IP(option 50)
    let opts = &pkt[240..];
    let mut found_rip = false;
    let mut found_sid = false;
    let mut i = 0;
    while i < opts.len() {
      if opts[i] == OPT_END {
        break;
      }
      if opts[i] == 0 {
        i += 1;
        continue;
      }
      let code = opts[i];
      let len = opts[i + 1] as usize;
      let val = &opts[i + 2..i + 2 + len];
      if code == 50 {
        assert_eq!(val, &requested_ip.octets());
        found_rip = true;
      }
      if code == OPT_SERVER_ID {
        assert_eq!(val, &server_id.octets());
        found_sid = true;
      }
      i += 2 + len;
    }
    assert!(found_rip, "requested IP option not found");
    assert!(found_sid, "server ID option not found");
  }

  #[test]
  fn dhcp_response_parsing() {
    let mut data = vec![0u8; 300];
    data[0] = 2;
    let xid: u32 = 0xCAFEBABE;
    data[4..8].copy_from_slice(&xid.to_be_bytes());

    data[16] = 10;
    data[17] = 0;
    data[18] = 0;
    data[19] = 50;

    data[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);

    let mut i = 240;

    data[i] = OPT_MSG_TYPE;
    data[i + 1] = 1;
    data[i + 2] = DHCP_ACK;
    i += 3;

    data[i] = OPT_SUBNET_MASK;
    data[i + 1] = 4;
    data[i + 2..i + 6].copy_from_slice(&[255, 255, 255, 0]);
    i += 6;

    data[i] = OPT_ROUTER;
    data[i + 1] = 4;
    data[i + 2..i + 6].copy_from_slice(&[10, 0, 0, 1]);
    i += 6;

    data[i] = OPT_DNS;
    data[i + 1] = 8;
    data[i + 2..i + 6].copy_from_slice(&[8, 8, 8, 8]);
    data[i + 6..i + 10].copy_from_slice(&[8, 8, 4, 4]);
    i += 10;

    data[i] = OPT_LEASE_TIME;
    data[i + 1] = 4;
    data[i + 2..i + 6].copy_from_slice(&3600u32.to_be_bytes());
    i += 6;

    data[i] = OPT_END;

    let resp = parse_dhcp_response(&data[..i + 1], xid).expect("should parse");
    assert_eq!(resp.msg_type, DHCP_ACK);
    assert_eq!(resp.your_ip, Ipv4Addr::new(10, 0, 0, 50));
    assert_eq!(resp.subnet_mask, Some(Ipv4Addr::new(255, 255, 255, 0)));
    assert_eq!(resp.router, Some(Ipv4Addr::new(10, 0, 0, 1)));
    assert_eq!(resp.dns_servers.len(), 2);
    assert_eq!(resp.dns_servers[0], Ipv4Addr::new(8, 8, 8, 8));
    assert_eq!(resp.dns_servers[1], Ipv4Addr::new(8, 8, 4, 4));
    assert_eq!(resp.lease_time, Some(3600));
  }

  #[test]
  fn resolv_conf_generation() {
    let servers = vec![Ipv4Addr::new(8, 8, 8, 8), Ipv4Addr::new(1, 1, 1, 1)];
    let content = generate_resolv_conf(&servers);
    assert!(content.contains("nameserver 8.8.8.8"));
    assert!(content.contains("nameserver 1.1.1.1"));
    assert!(content.starts_with("# Generated by rind"));
  }

  #[test]
  fn parse_mac_address() {
    let mac = parse_mac("52:54:00:12:34:56").unwrap();
    assert_eq!(mac, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

    assert!(parse_mac("invalid").is_none());
    assert!(parse_mac("52:54:00:12:34").is_none());
    assert!(parse_mac("52:54:00:12:34:56:78").is_none());
    assert!(parse_mac("GG:54:00:12:34:56").is_none());
  }

  #[test]
  fn parse_cidr_address() {
    let (ip, prefix) = parse_cidr("192.168.1.100/24").unwrap();
    assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 100));
    assert_eq!(prefix, 24);

    assert!(parse_cidr("192.168.1.100").is_none());
    assert!(parse_cidr("invalid/24").is_none());
    assert!(parse_cidr("192.168.1.100/33").is_none());
  }

  #[test]
  fn prefix_to_mask_conversion() {
    assert_eq!(prefix_to_mask(24), Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(prefix_to_mask(16), Ipv4Addr::new(255, 255, 0, 0));
    assert_eq!(prefix_to_mask(8), Ipv4Addr::new(255, 0, 0, 0));
    assert_eq!(prefix_to_mask(32), Ipv4Addr::new(255, 255, 255, 255));
    assert_eq!(prefix_to_mask(0), Ipv4Addr::UNSPECIFIED);
  }
}
