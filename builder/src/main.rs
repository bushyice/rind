use std::{
  collections::BTreeSet,
  collections::HashMap,
  ffi::CString,
  fs,
  io::{Read, Seek, SeekFrom},
  path::{Path, PathBuf},
  process::{exit, Command},
};

use std::os::unix::fs::MetadataExt;

use ext4_lwext4::{Ext4Fs, FileBlockDevice, OpenFlags};
use flate2::read::GzDecoder;
use reqwest::blocking::get;
use serde::Deserialize;
use xz2::read::XzDecoder;

fn set_ext4_mtime(target_path: &str, mtime: u64) {
  let mtime = if mtime > u32::MAX as u64 {
    u32::MAX
  } else {
    mtime as u32
  };
  for mp_idx in 0..16 {
    let full_path = format!("/mp{}{}", mp_idx, target_path);
    let Ok(c_path) = CString::new(full_path) else {
      continue;
    };
    if unsafe { ext4_lwext4_sys::ext4_mtime_set(c_path.as_ptr(), mtime) } == 0 {
      return;
    }
  }
}

#[derive(Debug, Deserialize)]
struct RinbConfig {
  profile: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UserGroup {
  Simple(String),
  Detailed { gid: u32, name: String },
}

#[derive(Debug, Deserialize)]
struct UserConfig {
  username: String,
  password: Option<String>,
  uid: u32,
  gid: u32,
  home: String,
  shell: String,
  groups: Option<Vec<UserGroup>>,
  env: Option<EnvConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EnvConfig {
  Map(HashMap<String, String>),
  List(Vec<String>),
}

impl EnvConfig {
  fn to_map(&self) -> HashMap<String, String> {
    match self {
      EnvConfig::Map(map) => map.clone(),
      EnvConfig::List(list) => {
        let mut map = HashMap::new();
        let mut iter = list.iter();
        while let Some(k) = iter.next() {
          if let Some(v) = iter.next() {
            map.insert(k.to_string(), v.to_string());
          }
        }
        map
      }
    }
  }
}

#[derive(Debug, Deserialize)]
struct Profile {
  build_command: Option<String>,
  binary_target: Option<String>,
  bins_dir: Option<String>,
  binaries: Option<Vec<String>>,
  files: Option<Vec<String>>,
  libs: Option<Vec<String>>,
  install: Option<Vec<String>>,
  // disk_mode: Option<String>,
  disk_size: Option<u64>,
  linux_image: Option<String>,
  run_options: Option<Vec<String>>,
  qemu_options: Option<QemuOptions>,
  nodes: Option<Vec<NodeConfig>>,
  busybox_url: Option<String>,
  busybox_applets: Option<Vec<String>>,
  symlinks: Option<Vec<String>>,
  ldd_bins: Option<Vec<String>>,
  #[serde(alias = "root_envs")]
  root_env: Option<EnvConfig>,
  user: Option<Vec<UserConfig>>,
  disk_permissions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct QemuOptions {
  arch: Option<String>,
  args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct NodeConfig {
  path: String,
  node_type: String,
  major: u64,
  minor: u64,
  mode: u32,
}

fn artifact_path() -> PathBuf {
  Path::new(".artifacts").to_path_buf()
}

fn configured_bins_dir(profile: &Profile) -> &str {
  profile.bins_dir.as_deref().unwrap_or("/usr/bin")
}

fn render_env_lines(map: &HashMap<String, String>) -> String {
  let mut keys: Vec<&String> = map.keys().collect();
  keys.sort();
  let mut out = String::new();
  for k in keys {
    out.push_str(&format!("{}={}\n", k, map.get(k).unwrap()));
  }
  out
}

fn cached_download(url: &str, name: Option<&str>) -> PathBuf {
  let filename = if let Some(name) = name {
    name
  } else {
    url.split('/').last().unwrap()
  };
  let path = artifact_path().join(filename);
  if path.exists() {
    println!("[*] Using cached {}", path.display());
    return path;
  }
  fs::create_dir_all(artifact_path()).unwrap();
  println!("[*] Downloading {url} -> {}", path.display());
  let resp = get(url).unwrap().bytes().unwrap();
  fs::write(&path, &resp).unwrap();
  path
}

fn parse_install_entry(entry: &str) -> (String, String) {
  if entry.starts_with("http") {
    let alias = entry
      .split('/')
      .last()
      .unwrap()
      .split('.')
      .next()
      .unwrap()
      .to_string();
    return (alias, entry.to_string());
  }
  let (alias, url) = entry.split_once(':').expect("Invalid install entry");
  (alias.to_string(), url.to_string())
}

fn read_ext4_text_file(fs_ext4: &mut Ext4Fs, path: &str) -> String {
  if !fs_ext4.exists(path) {
    return String::new();
  }
  let mut out = String::new();
  if let Ok(mut f) = fs_ext4.open(path, OpenFlags::READ) {
    f.read_to_string(&mut out).ok();
  }
  out
}

fn write_ext4_text_file(fs_ext4: &mut Ext4Fs, path: &str, content: &str) {
  if let Some(parent) = Path::new(path).parent() {
    let parent = parent.to_str().unwrap();
    if !parent.is_empty() && !fs_ext4.exists(parent) {
      fs_ext4.mkdir(parent, 0o755).ok();
    }
  }
  if let Ok(mut f) = fs_ext4.open(
    path,
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    println!("[*] Saving extraction into the lock file");
    f.write_all(content.as_bytes()).ok();
  }
}

fn read_builder_lock(fs_ext4: &mut Ext4Fs) -> HashMap<String, u64> {
  let mut map = HashMap::new();
  for line in read_ext4_text_file(fs_ext4, "/etc/builder.lock").lines() {
    let Some((key, value)) = line.rsplit_once('\t') else {
      continue;
    };
    if let Ok(ts) = value.parse::<u64>() {
      map.insert(key.to_string(), ts);
    }
  }
  map
}

fn write_builder_lock(fs_ext4: &mut Ext4Fs, lock: &HashMap<String, u64>) {
  let mut keys: Vec<&String> = lock.keys().collect();
  keys.sort();
  let mut content = String::new();
  for key in keys {
    content.push_str(&format!("{key}\t{}\n", lock.get(key).unwrap()));
  }
  write_ext4_text_file(fs_ext4, "/etc/builder.lock", &content);
}

fn extract_archive_to_ext4(
  fs_ext4: &mut Ext4Fs,
  archive: &Path,
  member: &str,
  dst_root: &str,
  lock: &mut HashMap<String, u64>,
) {
  let archive_mtime = fs::metadata(archive)
    .ok()
    .and_then(|m| m.modified().ok())
    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    .map(|d| d.as_secs())
    .unwrap_or(0);
  let lock_key = format!("{}\t{}\t{}", archive.display(), member, dst_root);
  if lock.get(&lock_key).copied() == Some(archive_mtime) {
    println!(
      "[*] Skipping extract (up-to-date): {} [{} -> {}]",
      archive.display(),
      member,
      dst_root
    );
    return;
  }

  let mut file = fs::File::open(archive).expect("Failed to open archive");
  let mut magic = [0u8; 4];
  let _ = file.read(&mut magic);
  file.seek(SeekFrom::Start(0)).unwrap();

  let reader: Box<dyn Read> = if magic[0..2] == [0x1f, 0x8b] {
    Box::new(GzDecoder::new(file))
  } else if magic[0..4] == [0xfd, 0x37, 0x7a, 0x58] {
    Box::new(XzDecoder::new(file))
  } else if magic[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
    Box::new(zstd::stream::read::Decoder::new(file).expect("Failed to create zstd decoder"))
  } else {
    Box::new(file)
  };

  println!("[*] Extracting: {member} from {}", archive.display());

  let mut archive = tar::Archive::new(reader);
  let member = member.trim_start_matches('/');
  let dst_root = dst_root.trim_end_matches('/');

  for entry in archive.entries().expect("Failed to read archive entries") {
    let mut entry = entry.expect("Failed to read entry");
    let path = entry
      .path()
      .expect("Failed to get entry path")
      .to_path_buf();
    let path_str = path.to_str().unwrap();
    if !path_str.starts_with(member) {
      continue;
    }
    let rel_path = &path_str[member.len()..].trim_start_matches('/');
    let target_path = if rel_path.is_empty() {
      dst_root.to_string()
    } else {
      format!("{}/{}", dst_root, rel_path)
    };

    if entry.header().entry_type().is_dir() {
      if !fs_ext4.exists(&target_path) {
        fs_ext4.mkdir(&target_path, 0o755).ok();
      }
    } else if entry.header().entry_type().is_file() {
      if let Some(parent) = Path::new(&target_path).parent() {
        if !fs_ext4.exists(parent.to_str().unwrap()) {
          fs_ext4.mkdir(parent.to_str().unwrap(), 0o755).ok();
        }
      }
      let mut content = Vec::new();
      entry.read_to_end(&mut content).ok();
      if let Ok(mut f) = fs_ext4.open(
        &target_path,
        OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
      ) {
        f.write_all(&content).ok();
        fs_ext4
          .set_permissions(&target_path, entry.header().mode().unwrap_or(0o644))
          .ok();
        if let Ok(mtime) = entry.header().mtime() {
          set_ext4_mtime(&target_path, mtime);
        }
      }
    } else if entry.header().entry_type().is_symlink() {
      if let Ok(Some(link)) = entry.link_name() {
        fs_ext4.symlink(link.to_str().unwrap(), &target_path).ok();
      }
    }
  }
  lock.insert(lock_key, archive_mtime);
}

fn copy_host_path_to_ext4(
  fs_ext4: &mut Ext4Fs,
  src: &Path,
  dst: &str,
  disk_perms: &HashMap<String, (u32, u32, u32)>,
) {
  let meta = match fs::symlink_metadata(src) {
    Ok(m) => m,
    Err(_) => return,
  };
  if meta.file_type().is_symlink() {
    fs_ext4
      .symlink(fs::read_link(src).unwrap().to_str().unwrap(), dst)
      .ok();
    return;
  }
  if meta.is_dir() {
    if !fs_ext4.exists(dst) {
      fs_ext4.mkdir(dst, 0o755).ok();
    }
    for entry in fs::read_dir(src).unwrap() {
      let entry = entry.unwrap();
      copy_host_path_to_ext4(
        fs_ext4,
        &entry.path(),
        &format!(
          "{}/{}",
          dst.trim_end_matches('/'),
          entry.file_name().to_str().unwrap()
        ),
        disk_perms,
      );
    }
    return;
  }
  let src_mtime = meta
    .modified()
    .unwrap()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs();
  if fs_ext4.exists(dst) && src_mtime <= fs_ext4.metadata(dst).unwrap().mtime {
    return;
  }
  if let Ok(mut f) = fs_ext4.open(
    dst,
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    println!("[*] Updating file: {dst}");
    f.write_all(&fs::read(src).unwrap()).ok();
    set_ext4_mtime(dst, src_mtime);
    let (uid, gid, mode) = match disk_perms.get(dst) {
      Some((u, g, m)) => {
        println!("  -> perm override {u}, {g}, {m}");
        (*u, *g, *m)
      }
      None => (0, 0, meta.mode()),
    };
    fs_ext4.set_owner(dst, uid, gid).ok();
    fs_ext4.set_permissions(dst, mode).ok();
  }
}

fn parse_ldd_output(line: &str) -> Option<String> {
  let trimmed = line.trim();
  if trimmed.is_empty() || trimmed.contains("statically linked") || trimmed.contains("not found") {
    return None;
  }
  if let Some((_, right)) = trimmed.split_once("=>") {
    let rhs = right.trim();
    if let Some(path) = rhs.split_whitespace().next() {
      if path.starts_with('/') {
        return Some(path.to_string());
      }
    }
    return None;
  }
  let path = trimmed.split_whitespace().next()?;
  if path.starts_with('/') {
    Some(path.to_string())
  } else {
    None
  }
}

fn copy_host_path_into_ext4(
  fs_ext4: &mut Ext4Fs,
  src: &Path,
  disk_perms: &HashMap<String, (u32, u32, u32)>,
) {
  if !src.is_absolute() {
    return;
  }
  let meta = match fs::symlink_metadata(src) {
    Ok(m) => m,
    Err(_) => return,
  };
  let dst = src.to_str().unwrap();
  if meta.file_type().is_symlink() {
    let link = fs::read_link(src).unwrap();
    fs_ext4.symlink(link.to_str().unwrap(), dst).ok();
    copy_host_path_into_ext4(
      fs_ext4,
      &if link.is_absolute() {
        link
      } else {
        src.parent().unwrap().join(link)
      },
      disk_perms,
    );
    return;
  }
  copy_host_path_to_ext4(fs_ext4, src, dst, disk_perms);
}

fn builder_b(profile: &Profile) {
  if let Some(cmd) = &profile.build_command {
    println!("[*] Running build command: {}", cmd);
    let status = Command::new("sh").args(&["-c", cmd]).status().unwrap();
    if !status.success() {
      exit(1);
    }
  }
}

fn parse_disk_perms(perms: &Vec<String>) -> HashMap<String, (u32, u32, u32)> {
  let mut map = HashMap::new();
  for perm in perms {
    let parts: Vec<&str> = perm.split(':').collect();
    if parts.len() < 3 {
      continue;
    }
    map.insert(
      parts[0].to_string(),
      (
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
        parts
          .get(3)
          .map(|x| u32::from_str_radix(x, 8).unwrap())
          .unwrap_or(0),
      ),
    );
  }
  map
}

fn prepare_rootfs(profile: &Profile, fs_ext4: &mut Ext4Fs) {
  let disk_perms = parse_disk_perms(profile.disk_permissions.as_ref().unwrap_or(&Vec::new()));
  for dir in &[
    "/etc",
    "/usr",
    "/var",
    "/root",
    "/home",
    "/tmp",
    "/usr/bin",
    "/usr/lib",
    "/usr/include",
  ] {
    if !fs_ext4.exists(dir) {
      fs_ext4.mkdir(dir, 0o755).ok();
    }
  }
  let bins_dir = configured_bins_dir(profile);
  if let Some(binaries) = &profile.binaries {
    for bin in binaries {
      let (bin_name, dst) = if let Some((left, right)) = bin.split_once(':') {
        let left_trim = left.trim_start_matches('/');
        if left_trim == "bin" || left_trim == "usr/bin" {
          (right, format!("/{left_trim}/{right}"))
        } else {
          (left, right.to_string())
        }
      } else {
        (bin.as_str(), format!("{}/{}", bins_dir, bin))
      };
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(bin_name);
      copy_host_path_to_ext4(fs_ext4, &src, &dst, &disk_perms);
    }
  }
  if let Some(libs) = &profile.libs {
    for lib in libs {
      let is_so = lib.starts_with('C');
      let parts: Vec<&str> = lib.trim_start_matches('C').splitn(3, ':').collect();
      if parts.len() != 3 {
        continue;
      }
      let libname = format!(
        "lib{}.{}",
        parts[0].replace('-', "_"),
        if is_so { "so" } else { "a" }
      );
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(&libname);
      copy_host_path_to_ext4(fs_ext4, &src, &format!("/usr/lib/{}", libname), &disk_perms);
      let mut buf = Vec::new();
      cbindgen::Builder::new()
        .with_crate(parts[2])
        .with_language(cbindgen::Language::C)
        .with_pragma_once(true)
        .with_cpp_compat(false)
        .generate()
        .unwrap()
        .write(&mut buf);
      if let Ok(mut f) = fs_ext4.open(
        &format!("/usr/include/{}.h", parts[1]),
        OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
      ) {
        f.write_all(&buf).ok();
      }
    }
  }
  if let Some(files) = &profile.files {
    for mapping in files {
      if mapping.starts_with('@') {
        continue;
      }
      let parts: Vec<&str> = mapping.splitn(2, ':').collect();
      if parts.len() != 2 {
        continue;
      }
      let dst = if parts[1].starts_with('/') {
        parts[1].to_string()
      } else {
        format!("/{}", parts[1])
      };
      copy_host_path_to_ext4(fs_ext4, Path::new(parts[0]), &dst, &disk_perms);
    }
  }
  if let Some(symlinks) = &profile.symlinks {
    for mapping in symlinks {
      let parts: Vec<&str> = mapping.splitn(2, ':').collect();
      if parts.len() == 2 {
        if let Some(parent) = Path::new(parts[1]).parent() {
          if !fs_ext4.exists(parent.to_str().unwrap()) {
            fs_ext4.mkdir(parent.to_str().unwrap(), 0o755).ok();
          }
        }
        fs_ext4.symlink(parts[0], parts[1]).ok();
      }
    }
  }
  if let Some(bins) = &profile.ldd_bins {
    let mut deps: BTreeSet<String> = BTreeSet::new();
    for bin in bins {
      if let Ok(output) = Command::new("ldd").arg(bin).output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
          if let Some(path) = parse_ldd_output(line) {
            deps.insert(path);
          }
        }
      }
    }
    for dep in deps {
      copy_host_path_into_ext4(fs_ext4, Path::new(&dep), &disk_perms);
    }
  }
}

fn builder_n(profile: &Profile, fs_ext4: &mut Ext4Fs) {
  if let Some(nodes) = &profile.nodes {
    for node in nodes {
      if fs_ext4.exists(&node.path) {
        continue;
      }
      let mode = (match node.node_type.as_str() {
        "c" => 0x2000,
        "b" => 0x6000,
        _ => 0,
      }) | node.mode;
      let dev = (node.major << 8) | node.minor;
      unsafe {
        ext4_lwext4_sys::ext4_mknod(
          CString::new(format!("/mp0{}", node.path)).unwrap().as_ptr(),
          mode as i32,
          dev as u32,
        );
      }
    }
  }
}

fn builder_i(profile: &Profile, fs_ext4: &mut Ext4Fs) {
  let mut extract_lock = read_builder_lock(fs_ext4);
  let mut archive_cache: HashMap<String, PathBuf> = HashMap::new();
  if let Some(installs) = &profile.install {
    for entry in installs {
      let (alias, url) = parse_install_entry(entry);
      archive_cache.insert(
        alias,
        cached_download(
          &url,
          Some(&format!(
            "{}__{}",
            entry.split(':').next().unwrap(),
            url.split('/').last().unwrap()
          )),
        ),
      );
    }
  }
  if let Some(files) = &profile.files {
    for mapping in files {
      if !mapping.starts_with('@') {
        continue;
      }
      let parts: Vec<&str> = mapping.splitn(3, ':').collect();
      if parts.len() == 3 {
        let alias = parts[0].trim_start_matches('@');
        let dst = if parts[2].starts_with('/') {
          parts[2].to_string()
        } else {
          format!("/{}", parts[2])
        };
        if let Some(archive) = archive_cache.get(alias) {
          extract_archive_to_ext4(fs_ext4, archive, parts[1], &dst, &mut extract_lock);
        }
      }
    }
  }
  write_builder_lock(fs_ext4, &extract_lock);
  if let Some(kernel_url) = &profile.linux_image {
    cached_download(
      kernel_url.split_once(':').unwrap().1,
      Some(kernel_url.split_once(':').unwrap().0),
    );
  }
  if let Some(url) = &profile.busybox_url {
    let bb_path = cached_download(url, None);
    let bins_dir = configured_bins_dir(profile);
    let busybox_dst = format!("{}/busybox", bins_dir);
    if let Ok(mut f) = fs_ext4.open(
      &busybox_dst,
      OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
    ) {
      f.write_all(&fs::read(bb_path).unwrap()).ok();
      fs_ext4.set_permissions(&busybox_dst, 0o755).ok();
    }
    for app in profile.busybox_applets.clone().unwrap_or_else(|| {
      vec![
        "[",
        "[[",
        "]]",
        "]",
        "acpid",
        "add-shell",
        "addgroup",
        "adduser",
        "adjtimex",
        "arp",
        "arping",
        "ash",
        "awk",
        "base64",
        "basename",
        "beep",
        "blkid",
        "blockdev",
        "bootchartd",
        "brctl",
        "bunzip2",
        "bzcat",
        "bzip2",
        "cal",
        "cat",
        "catv",
        "chat",
        "chattr",
        "chgrp",
        "chmod",
        "chown",
        "chpasswd",
        "chpst",
        "chroot",
        "chrt",
        "chvt",
        "cksum",
        "clear",
        "cmp",
        "comm",
        "cp",
        "cpio",
        "crond",
        "crontab",
        "cryptpw",
        "cttyhack",
        "cut",
        "date",
        "dc",
        "dd",
        "deallocvt",
        "delgroup",
        "deluser",
        "depmod",
        "devmem",
        "df",
        "dhcprelay",
        "diff",
        "dirname",
        "dmesg",
        "dnsd",
        "dnsdomainname",
        "dos2unix",
        "du",
        "dumpkmap",
        "dumpleases",
        "echo",
        "ed",
        "egrep",
        "eject",
        "env",
        "envdir",
        "envuidgid",
        "ether-wake",
        "expand",
        "expr",
        "fakeidentd",
        "false",
        "fbset",
        "fbsplash",
        "fdflush",
        "fdformat",
        "fdisk",
        "fgconsole",
        "fgrep",
        "find",
        "findfs",
        "flock",
        "fold",
        "free",
        "freeramdisk",
        "fsck",
        "fsck.minix",
        "fsync",
        "ftpd",
        "ftpget",
        "ftpput",
        "fuser",
        "getopt",
        "getty",
        "grep",
        "groups",
        "gunzip",
        "gzip",
        "halt",
        "hd",
        "hdparm",
        "head",
        "hexdump",
        "hostid",
        "hostname",
        "httpd",
        "hush",
        "hwclock",
        "id",
        "ifconfig",
        "ifdown",
        "ifenslave",
        "ifplugd",
        "ifup",
        "inetd",
        "insmod",
        "install",
        "ionice",
        "iostat",
        "ip",
        "ipaddr",
        "ipcalc",
        "ipcrm",
        "ipcs",
        "iplink",
        "iproute",
        "iprule",
        "iptunnel",
        "kbd_mode",
        "kill",
        "killall",
        "killall5",
        "klogd",
        "last",
        "less",
        "linux32",
        "linux64",
        "linuxrc",
        "ln",
        "loadfont",
        "loadkmap",
        "logger",
        "login",
        "logname",
        "logread",
        "losetup",
        "lpd",
        "lpq",
        "lpr",
        "ls",
        "lsattr",
        "lsmod",
        "lspci",
        "lsusb",
        "lzcat",
        "lzma",
        "lzop",
        "lzopcat",
        "makedevs",
        "makemime",
        "man",
        "md5sum",
        "mdev",
        "mesg",
        "microcom",
        "mkdir",
        "mkdosfs",
        "mke2fs",
        "mkfifo",
        "mkfs.ext2",
        "mkfs.minix",
        "mkfs.vfat",
        "mknod",
        "mkpasswd",
        "mkswap",
        "mktemp",
        "modinfo",
        "modprobe",
        "more",
        "mount",
        "mountpoint",
        "mpstat",
        "mt",
        "mv",
        "nameif",
        "nbd-client",
        "nc",
        "netstat",
        "nice",
        "nmeter",
        "nohup",
        "nslookup",
        "ntpd",
        "od",
        "openvt",
        "passwd",
        "patch",
        "pgrep",
        "pidof",
        "ping",
        "ping6",
        "pipe_progress",
        "pivot_root",
        "pkill",
        "pmap",
        "popmaildir",
        "poweroff",
        "powertop",
        "printenv",
        "printf",
        "ps",
        "pscan",
        "pstree",
        "pwd",
        "pwdx",
        "raidautorun",
        "rdate",
        "rdev",
        "readahead",
        "readlink",
        "readprofile",
        "realpath",
        "reboot",
        "reformime",
        "remove-shell",
        "renice",
        "reset",
        "resize",
        "rev",
        "rm",
        "rmdir",
        "rmmod",
        "route",
        "rpm",
        "rpm2cpio",
        "rtcwake",
        "run-parts",
        "runlevel",
        "runsv",
        "runsvdir",
        "rx",
        "script",
        "scriptreplay",
        "sed",
        "sendmail",
        "seq",
        "setarch",
        "setconsole",
        "setfont",
        "setkeycodes",
        "setlogcons",
        "setserial",
        "setsid",
        "setuidgid",
        "sh",
        "sha1sum",
        "sha256sum",
        "sha512sum",
        "showkey",
        "slattach",
        "sleep",
        "smemcap",
        "softlimit",
        "sort",
        "split",
        "start-stop-daemon",
        "stat",
        "strings",
        "stty",
        "su",
        "sulogin",
        "sum",
        "sv",
        "svlogd",
        "swapoff",
        "swapon",
        "switch_root",
        "sync",
        "sysctl",
        "syslogd",
        "tac",
        "tail",
        "tar",
        "tcpsvd",
        "tee",
        "telnet",
        "telnetd",
        "test",
        "tftp",
        "tftpd",
        "time",
        "timeout",
        "top",
        "touch",
        "tr",
        "traceroute",
        "traceroute6",
        "true",
        "tty",
        "ttysize",
        "tunctl",
        "ubiattach",
        "ubidetach",
        "ubimkvol",
        "ubirmvol",
        "ubirsvol",
        "ubiupdatevol",
        "udhcpc",
        "udhcpd",
        "udpsvd",
        "umount",
        "uname",
        "unexpand",
        "uniq",
        "unix2dos",
        "unlzma",
        "unlzop",
        "unxz",
        "unzip",
        "uptime",
        "users",
        "usleep",
        "uudecode",
        "uuencode",
        "vconfig",
        "vi",
        "vlock",
        "volname",
        "wall",
        "watch",
        "watchdog",
        "wc",
        "wget",
        "which",
        "who",
        "whoami",
        "whois",
        "xargs",
        "xz",
        "xzcat",
        "yes",
        "zcat",
        "zcip",
      ]
      .iter()
      .map(|x| x.to_string())
      .collect()
    }) {
      fs_ext4
        .symlink("busybox", &format!("{}/{}", bins_dir, app))
        .ok();
    }
  }
}

fn builder_u(profile: &Profile, fs_ext4: &mut Ext4Fs) {
  let mut passwd = String::from("root:x:0:0:root:/root:/bin/sh\n");
  let mut shadow = String::from("root:*:19000:0:99999:7:::\n");
  let mut group_map: HashMap<String, (u32, Vec<String>)> = HashMap::new();
  group_map.insert("root".into(), (0, vec!["root".into()]));
  group_map.insert("wheel".into(), (10, vec!["root".into()]));
  if let Some(root_env) = &profile.root_env {
    if let Ok(mut f) = fs_ext4.open(
      "/etc/.env",
      OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
    ) {
      f.write_all(render_env_lines(&root_env.to_map()).as_bytes())
        .ok();
    }
  }
  if let Some(users) = &profile.user {
    for user in users {
      let home = if user.username == "root" {
        "/root".to_string()
      } else {
        user.home.clone()
      };
      if !fs_ext4.exists(&home) {
        fs_ext4.mkdir(&home, 0o755).ok();
      }
      if let Some(env) = &user.env {
        if let Ok(mut f) = fs_ext4.open(
          &format!("{}/.env", home),
          OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        ) {
          f.write_all(render_env_lines(&env.to_map()).as_bytes()).ok();
        }
      }
      if user.username != "root" {
        passwd.push_str(&format!(
          "{}:x:{}:{}:User:{}:{}\n",
          user.username, user.uid, user.gid, user.home, user.shell
        ));
        let hash = if let Some(p) = &user.password {
          sha_crypt::sha512_simple(p, &sha_crypt::Sha512Params::new(5000).unwrap())
            .unwrap_or_else(|_| "*".to_string())
        } else {
          "*".to_string()
        };
        shadow.push_str(&format!("{}:{}:19000:0:99999:7:::\n", user.username, hash));

        group_map
          .entry(user.username.clone())
          .or_insert((user.gid, Vec::new()))
          .1
          .push(user.username.clone());

        if let Some(groups) = &user.groups {
          for g in groups {
            let (g, gid) = match g {
              UserGroup::Simple(g) => (g.clone(), 1000u32),
              UserGroup::Detailed { gid, name } => (name.clone(), *gid),
            };
            group_map
              .entry(g)
              .or_insert((gid, Vec::new()))
              .1
              .push(user.username.clone());
          }
        }
      }
      fs_ext4.set_owner(&home, user.uid, user.gid).ok();
    }
  }
  if let Ok(mut f) = fs_ext4.open(
    "/etc/passwd",
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    f.write_all(passwd.as_bytes()).ok();
  }
  if let Ok(mut f) = fs_ext4.open(
    "/etc/shadow",
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    f.write_all(shadow.as_bytes()).ok();
    fs_ext4.set_permissions("/etc/shadow", 0o600).ok();
  }
  if let Ok(mut f) = fs_ext4.open(
    "/etc/group",
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    let mut group_str = String::new();
    for (name, (gid, members)) in group_map {
      group_str.push_str(&format!("{}:x:{}:{}\n", name, gid, members.join(",")));
    }
    f.write_all(group_str.as_bytes()).ok();
  }
}

fn run(profile: &Profile) {
  let kernel_path = artifact_path().join(
    profile
      .linux_image
      .as_ref()
      .unwrap()
      .split(':')
      .next()
      .unwrap(),
  );
  let qemu_options = if let Some(qemu_options) = &profile.qemu_options {
    qemu_options
  } else {
    &QemuOptions {
      arch: Some("x86_64".to_string()),
      args: None,
    }
  };

  let mut cmd = Command::new(if let Some(arch) = &qemu_options.arch {
    format!("qemu-system-{}", arch)
  } else {
    "qemu-system-x86_64".to_string()
  });

  cmd
    .arg("-kernel")
    .arg(kernel_path)
    .arg("-drive")
    .arg(format!(
      "file={},format=raw,if=virtio",
      artifact_path().join("rootfs.img").display()
    ));
  if let Some(opt) = &profile.run_options {
    cmd.arg("-append").arg(opt.join(" "));
  }
  if let Some(args) = &qemu_options.args {
    cmd.args(args);
  }
  cmd.status().ok();
}

fn handle_command(c: &str, profile: &Profile, fs_ext4: &mut Option<Ext4Fs>, no_overwrite: bool) {
  match c {
    "b" => {
      if let Some(cmd) = &profile.build_command {
        println!("[*] Running build command: {}", cmd);
        if !Command::new("sh")
          .args(&["-c", cmd])
          .status()
          .unwrap()
          .success()
        {
          exit(1);
        }
      }
    }
    "r" => run(profile),
    _ => {
      if fs_ext4.is_none() {
        let output = artifact_path().join("rootfs.img");
        if !no_overwrite || !output.exists() {
          println!("[*] Creating new ext4 disk image...");
          FileBlockDevice::create(&output, 1024 * 1024 * profile.disk_size.unwrap_or(1024))
            .expect("Failed to create image file");
          let status = Command::new("mkfs.ext4")
            .args(&[
              "-F",
              "-O",
              "^64bit,^metadata_csum,^flex_bg,^huge_file,^dir_index,^extent",
              "-E",
              "lazy_itable_init=0,lazy_journal_init=0",
              "-m",
              "0",
              output.to_str().unwrap(),
            ])
            .status()
            .expect("Failed to execute mkfs.ext4. Is e2fsprogs installed?");
          if !status.success() {
            panic!("mkfs.ext4 failed with status: {}", status);
          }
        }
        let device = FileBlockDevice::open(&output).expect("Failed to open image file");
        let mut fs = Ext4Fs::mount(device, false)
          .map_err(|e| {
            eprintln!(
              "[!] Failed to mount ext4 image: {:?}. Try deleting .artifacts/rootfs.img",
              e
            );
            e
          })
          .expect("Mount failed");
        if fs.is_read_only() {
          let _ = fs.umount();
          Command::new("e2fsck")
            .args(&["-p", "-f", output.to_str().unwrap()])
            .status()
            .ok();
          fs = Ext4Fs::mount(FileBlockDevice::open(&output).unwrap(), false).unwrap();
        }
        *fs_ext4 = Some(fs);
      }
      let fs = fs_ext4.as_mut().unwrap();
      match c {
        "n" => builder_n(profile, fs),
        "i" => builder_i(profile, fs),
        "p" => prepare_rootfs(profile, fs),
        "u" => builder_u(profile, fs),
        "a" => {
          builder_b(profile);
          builder_i(profile, fs);
          prepare_rootfs(profile, fs);
          builder_n(profile, fs);
          builder_u(profile, fs);
        }
        _ => {}
      }
    }
  }
}

fn main() {
  let args: Vec<String> = std::env::args().collect();
  if args.len() < 2 {
    eprintln!("Usage: builder <builder_command>");
    eprintln!("Commands: a, b, n, i, p, r, u");
    eprintln!("Examples:");
    eprintln!("build all: a");
    eprintln!("build cargo: b");
    eprintln!("create nodes: n");
    eprintln!("prepare rootfs: p");
    eprintln!("install urls: i");
    eprintln!("build disk: d");
    eprintln!("make users: u");
    eprintln!("run: r");
    eprintln!("use existing disk: x");
    eprintln!(
      "you can use multiple commands, for example this builds cargo, prepares disk and runs: bpr"
    );
    exit(1);
  }
  let config: RinbConfig = toml::from_str(&fs::read_to_string("builder.toml").unwrap()).unwrap();
  let profile = config.profile.get("main").unwrap();
  let mut fs_ext4: Option<Ext4Fs> = None;
  for c in args[1].chars() {
    if c == 'x' {
      continue;
    }
    handle_command(&c.to_string(), profile, &mut fs_ext4, args[1].contains('x'));
  }
  if let Some(fs) = fs_ext4 {
    let _ = fs.umount();
  }
}
