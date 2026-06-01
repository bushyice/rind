use std::ffi::CString;

pub fn bring_interface_up(iface: &str) -> Result<(), std::io::Error> {
  let c_iface = CString::new(iface).map_err(|_| {
    std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      "Interface name contains a null byte",
    )
  })?;

  let bytes = c_iface.as_bytes_with_nul();

  if bytes.len() > 16 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      "Interface name too long",
    ));
  }

  unsafe {
    let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
    if sock < 0 {
      return Err(std::io::Error::last_os_error());
    }

    let mut ifr: libc::ifreq = std::mem::zeroed();

    let target = &mut *(ifr.ifr_name.as_mut_ptr() as *mut [u8; 16]);
    target[..bytes.len()].copy_from_slice(bytes);

    if libc::ioctl(sock, libc::SIOCGIFFLAGS as _, &mut ifr) != 0 {
      let err = std::io::Error::last_os_error();
      libc::close(sock);
      return Err(err);
    }

    ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as i16;

    if libc::ioctl(sock, libc::SIOCSIFFLAGS as _, &ifr) != 0 {
      let err = std::io::Error::last_os_error();
      libc::close(sock);
      return Err(err);
    }

    // 7. Clean up resource
    libc::close(sock);
  }

  Ok(())
}

pub fn setup_namespace_loopback() {
  let _ = bring_interface_up("lo");
}
