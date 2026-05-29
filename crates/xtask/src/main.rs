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
  let status = Command::new("curl")
    .args(&["-L", "-o", path.to_str().unwrap(), url])
    .status()
    .expect("Failed to execute curl. Is curl installed?");
  if !status.success() {
    panic!("Failed to download URL: {}", url);
  }
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

  match fs_ext4.open(
    dst,
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
  ) {
    Ok(mut f) => {
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
    Err(e) => println!("Failed to open file: {e}"),
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
    "/usr/lib/rind",
    "/usr/lib/rind/plugins",
    "/usr/include",
  ] {
    if !fs_ext4.exists(dir) {
      match fs_ext4.mkdir(dir, 0o755) {
        Err(e) => println!("Failed to create folder: {e}"),
        Ok(_) => {}
      }
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
          (
            left,
            if right.ends_with("/") {
              format!("{right}{left}")
            } else {
              right.to_string()
            },
          )
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
      let config = cbindgen::Config {
        enumeration: cbindgen::EnumConfig {
          prefix_with_name: true,
          ..Default::default()
        },
        ..Default::default()
      };
      cbindgen::Builder::new()
        .with_config(config)
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
        "[".to_string(),
        "[[".to_string(),
        "]]".to_string(),
        "]".to_string(),
        "acpid".to_string(),
        "add-shell".to_string(),
        "addgroup".to_string(),
        "adduser".to_string(),
        "adjtimex".to_string(),
        "arp".to_string(),
        "arping".to_string(),
        "ash".to_string(),
        "awk".to_string(),
        "base64".to_string(),
        "basename".to_string(),
        "beep".to_string(),
        "blkid".to_string(),
        "blockdev".to_string(),
        "bootchartd".to_string(),
        "brctl".to_string(),
        "bunzip2".to_string(),
        "bzcat".to_string(),
        "bzip2".to_string(),
        "cal".to_string(),
        "cat".to_string(),
        "catv".to_string(),
        "chat".to_string(),
        "chattr".to_string(),
        "chgrp".to_string(),
        "chmod".to_string(),
        "chown".to_string(),
        "chpasswd".to_string(),
        "chpst".to_string(),
        "chroot".to_string(),
        "chrt".to_string(),
        "chvt".to_string(),
        "cksum".to_string(),
        "clear".to_string(),
        "cmp".to_string(),
        "comm".to_string(),
        "cp".to_string(),
        "cpio".to_string(),
        "crond".to_string(),
        "crontab".to_string(),
        "cryptpw".to_string(),
        "cttyhack".to_string(),
        "cut".to_string(),
        "date".to_string(),
        "dc".to_string(),
        "dd".to_string(),
        "deallocvt".to_string(),
        "delgroup".to_string(),
        "deluser".to_string(),
        "depmod".to_string(),
        "devmem".to_string(),
        "df".to_string(),
        "dhcprelay".to_string(),
        "diff".to_string(),
        "dirname".to_string(),
        "dmesg".to_string(),
        "dnsd".to_string(),
        "dnsdomainname".to_string(),
        "dos2unix".to_string(),
        "du".to_string(),
        "dumpkmap".to_string(),
        "dumpleases".to_string(),
        "echo".to_string(),
        "ed".to_string(),
        "egrep".to_string(),
        "eject".to_string(),
        "env".to_string(),
        "envdir".to_string(),
        "envuidgid".to_string(),
        "ether-wake".to_string(),
        "expand".to_string(),
        "expr".to_string(),
        "fakeidentd".to_string(),
        "false".to_string(),
        "fbset".to_string(),
        "fbsplash".to_string(),
        "fdflush".to_string(),
        "fdformat".to_string(),
        "fdisk".to_string(),
        "fgconsole".to_string(),
        "fgrep".to_string(),
        "find".to_string(),
        "findfs".to_string(),
        "flock".to_string(),
        "fold".to_string(),
        "free".to_string(),
        "freeramdisk".to_string(),
        "fsck".to_string(),
        "fsck.minix".to_string(),
        "fsync".to_string(),
        "ftpd".to_string(),
        "ftpget".to_string(),
        "ftpput".to_string(),
        "fuser".to_string(),
        "getopt".to_string(),
        "getty".to_string(),
        "grep".to_string(),
        "groups".to_string(),
        "gunzip".to_string(),
        "gzip".to_string(),
        "halt".to_string(),
        "hd".to_string(),
        "hdparm".to_string(),
        "head".to_string(),
        "hexdump".to_string(),
        "hostid".to_string(),
        "hostname".to_string(),
        "httpd".to_string(),
        "hush".to_string(),
        "hwclock".to_string(),
        "id".to_string(),
        "ifconfig".to_string(),
        "ifdown".to_string(),
        "ifenslave".to_string(),
        "ifplugd".to_string(),
        "ifup".to_string(),
        "inetd".to_string(),
        "insmod".to_string(),
        "install".to_string(),
        "ionice".to_string(),
        "iostat".to_string(),
        "ip".to_string(),
        "ipaddr".to_string(),
        "ipcalc".to_string(),
        "ipcrm".to_string(),
        "ipcs".to_string(),
        "iplink".to_string(),
        "iproute".to_string(),
        "iprule".to_string(),
        "iptunnel".to_string(),
        "kbd_mode".to_string(),
        "kill".to_string(),
        "killall".to_string(),
        "killall5".to_string(),
        "klogd".to_string(),
        "last".to_string(),
        "less".to_string(),
        "linux32".to_string(),
        "linux64".to_string(),
        "linuxrc".to_string(),
        "ln".to_string(),
        "loadfont".to_string(),
        "loadkmap".to_string(),
        "logger".to_string(),
        "login".to_string(),
        "logname".to_string(),
        "logread".to_string(),
        "losetup".to_string(),
        "lpd".to_string(),
        "lpq".to_string(),
        "lpr".to_string(),
        "ls".to_string(),
        "lsattr".to_string(),
        "lsmod".to_string(),
        "lspci".to_string(),
        "lsusb".to_string(),
        "lzcat".to_string(),
        "lzma".to_string(),
        "lzop".to_string(),
        "lzopcat".to_string(),
        "makedevs".to_string(),
        "makemime".to_string(),
        "man".to_string(),
        "md5sum".to_string(),
        "mdev".to_string(),
        "mesg".to_string(),
        "microcom".to_string(),
        "mkdir".to_string(),
        "mkdosfs".to_string(),
        "mke2fs".to_string(),
        "mkfifo".to_string(),
        "mkfs.ext2".to_string(),
        "mkfs.minix".to_string(),
        "mkfs.vfat".to_string(),
        "mknod".to_string(),
        "mkpasswd".to_string(),
        "mkswap".to_string(),
        "mktemp".to_string(),
        "modinfo".to_string(),
        "modprobe".to_string(),
        "more".to_string(),
        "mount".to_string(),
        "mountpoint".to_string(),
        "mpstat".to_string(),
        "mt".to_string(),
        "mv".to_string(),
        "nameif".to_string(),
        "nbd-client".to_string(),
        "nc".to_string(),
        "netstat".to_string(),
        "nice".to_string(),
        "nmeter".to_string(),
        "nohup".to_string(),
        "nslookup".to_string(),
        "ntpd".to_string(),
        "od".to_string(),
        "openvt".to_string(),
        "passwd".to_string(),
        "patch".to_string(),
        "pgrep".to_string(),
        "pidof".to_string(),
        "ping".to_string(),
        "ping6".to_string(),
        "pipe_progress".to_string(),
        "pivot_root".to_string(),
        "pkill".to_string(),
        "pmap".to_string(),
        "popmaildir".to_string(),
        "poweroff".to_string(),
        "powertop".to_string(),
        "printenv".to_string(),
        "printf".to_string(),
        "ps".to_string(),
        "pscan".to_string(),
        "pstree".to_string(),
        "pwd".to_string(),
        "pwdx".to_string(),
        "raidautorun".to_string(),
        "rdate".to_string(),
        "rdev".to_string(),
        "readahead".to_string(),
        "readlink".to_string(),
        "readprofile".to_string(),
        "realpath".to_string(),
        "reboot".to_string(),
        "reformime".to_string(),
        "remove-shell".to_string(),
        "renice".to_string(),
        "reset".to_string(),
        "resize".to_string(),
        "rev".to_string(),
        "rm".to_string(),
        "rmdir".to_string(),
        "rmmod".to_string(),
        "route".to_string(),
        "rpm".to_string(),
        "rpm2cpio".to_string(),
        "rtcwake".to_string(),
        "run-parts".to_string(),
        "runlevel".to_string(),
        "runsv".to_string(),
        "runsvdir".to_string(),
        "rx".to_string(),
        "script".to_string(),
        "scriptreplay".to_string(),
        "sed".to_string(),
        "sendmail".to_string(),
        "seq".to_string(),
        "setarch".to_string(),
        "setconsole".to_string(),
        "setfont".to_string(),
        "setkeycodes".to_string(),
        "setlogcons".to_string(),
        "setserial".to_string(),
        "setsid".to_string(),
        "setuidgid".to_string(),
        "sh".to_string(),
        "sha1sum".to_string(),
        "sha256sum".to_string(),
        "sha512sum".to_string(),
        "showkey".to_string(),
        "slattach".to_string(),
        "sleep".to_string(),
        "smemcap".to_string(),
        "softlimit".to_string(),
        "sort".to_string(),
        "split".to_string(),
        "start-stop-daemon".to_string(),
        "stat".to_string(),
        "strings".to_string(),
        "stty".to_string(),
        "su".to_string(),
        "sulogin".to_string(),
        "sum".to_string(),
        "sv".to_string(),
        "svlogd".to_string(),
        "swapoff".to_string(),
        "swapon".to_string(),
        "switch_root".to_string(),
        "sync".to_string(),
        "sysctl".to_string(),
        "syslogd".to_string(),
        "tac".to_string(),
        "tail".to_string(),
        "tar".to_string(),
        "tcpsvd".to_string(),
        "tee".to_string(),
        "telnet".to_string(),
        "telnetd".to_string(),
        "test".to_string(),
        "tftp".to_string(),
        "tftpd".to_string(),
        "time".to_string(),
        "timeout".to_string(),
        "top".to_string(),
        "touch".to_string(),
        "tr".to_string(),
        "traceroute".to_string(),
        "traceroute6".to_string(),
        "true".to_string(),
        "tty".to_string(),
        "ttysize".to_string(),
        "tunctl".to_string(),
        "ubiattach".to_string(),
        "ubidetach".to_string(),
        "ubimkvol".to_string(),
        "ubirmvol".to_string(),
        "ubirsvol".to_string(),
        "ubiupdatevol".to_string(),
        "udhcpc".to_string(),
        "udhcpd".to_string(),
        "udpsvd".to_string(),
        "umount".to_string(),
        "uname".to_string(),
        "unexpand".to_string(),
        "uniq".to_string(),
        "unix2dos".to_string(),
        "unlzma".to_string(),
        "unlzop".to_string(),
        "unxz".to_string(),
        "unzip".to_string(),
        "uptime".to_string(),
        "users".to_string(),
        "usleep".to_string(),
        "uudecode".to_string(),
        "uuencode".to_string(),
        "vconfig".to_string(),
        "vi".to_string(),
        "vlock".to_string(),
        "volname".to_string(),
        "wall".to_string(),
        "watch".to_string(),
        "watchdog".to_string(),
        "wc".to_string(),
        "wget".to_string(),
        "which".to_string(),
        "who".to_string(),
        "whoami".to_string(),
        "whois".to_string(),
        "xargs".to_string(),
        "xz".to_string(),
        "xzcat".to_string(),
        "yes".to_string(),
        "zcat".to_string(),
        "zcip".to_string(),
      ]
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
      "file={},format=raw,if=virtio,cache=none",
      artifact_path().join("rootfs.img").display()
    ));
  if let Some(opt) = &profile.run_options {
    cmd.arg("-append").arg(opt.join(" "));
  }
  if let Some(args) = &qemu_options.args {
    cmd.args(args);
  }
  println!(
    "[*] Running qemu as {} with args {:?}",
    qemu_options.arch.as_ref().unwrap_or(&"x86_64".to_string()),
    qemu_options.args
  );
  cmd.status().ok();
}

fn handle_command(c: &str, profile: &Profile, fs_ext4: &mut Option<Ext4Fs>, _no_overwrite: bool) {
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
    "r" => {
      if let Some(fs) = std::mem::take(fs_ext4) {
        let _ = fs.umount();
      }
      run(profile);
    }
    _ => {
      if fs_ext4.is_none() {
        let output = artifact_path().join("rootfs.img");
        if !output.exists() {
          println!("[*] Creating new ext4 disk image...");
          fs::create_dir_all(artifact_path()).ok();
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
        if fs.is_read_only()
          || matches!(fs.open("/.test", OpenFlags::CREATE | OpenFlags::WRITE), Err(e) if e.to_string().trim() == "read-only filesystem")
        {
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

fn print_usage() {
  eprintln!("Usage: cargo xtask <command> [args]");
  eprintln!("\nUnified Subcommands:");
  eprintln!("  mount / mr           Mount loopback device to .artifacts/mnt");
  eprintln!("  umount / umr         Unmount .artifacts/mnt");
  eprintln!("  clean-state          Mount, delete .artifacts/mnt/var/lib/system-state, and unmount");
  eprintln!("  test                 Run nextest or cargo test");
  eprintln!("  bench [bench_name]   Run cargo bench quiet");
  eprintln!("\nLegacy Builder Commands:");
  eprintln!("  a                    Build all");
  eprintln!("  b                    Run cargo build");
  eprintln!("  n                    Create nodes");
  eprintln!("  i                    Install urls (extract archives/download kernel)");
  eprintln!("  p                    Prepare rootfs");
  eprintln!("  r                    Run QEMU");
  eprintln!("  u                    Make users");
  eprintln!("  x                    Use existing disk (no overwrite)");
  eprintln!("\nLegacy Combination Example:");
  eprintln!("  cargo xtask xbpr     Builds cargo, prepares rootfs, and runs QEMU on existing disk");
}

fn mount_rootfs() {
  println!("[*] Creating mount directory .artifacts/mnt");
  fs::create_dir_all(".artifacts/mnt").unwrap();
  println!("[*] Mounting .artifacts/rootfs.img to .artifacts/mnt");
  let status = Command::new("sudo")
    .args(&["mount", "-o", "loop", ".artifacts/rootfs.img", ".artifacts/mnt"])
    .status()
    .expect("Failed to execute sudo mount");
  if !status.success() {
    eprintln!("[!] Failed to mount rootfs.img");
    exit(1);
  }
  println!("[*] Mounted successfully.");
}

fn umount_rootfs() {
  println!("[*] Unmounting .artifacts/mnt");
  let status = Command::new("sudo")
    .args(&["umount", ".artifacts/mnt"])
    .status()
    .expect("Failed to execute sudo umount");
  if !status.success() {
    eprintln!("[!] Failed to unmount .artifacts/mnt");
    exit(1);
  }
  println!("[*] Unmounted successfully.");
}

fn clean_state() {
  mount_rootfs();
  println!("[*] Cleaning state in .artifacts/mnt/var/lib/system-state");
  let status = Command::new("sudo")
    .args(&["rm", "-rf", ".artifacts/mnt/var/lib/system-state"])
    .status()
    .expect("Failed to delete system-state");
  if !status.success() {
    eprintln!("[!] Failed to clean system-state");
  }
  umount_rootfs();
}

fn run_tests() {
  println!("[*] Running tests...");
  let nextest_check = Command::new("cargo")
    .args(&["nextest", "--version"])
    .output();
  let status = if nextest_check.is_ok() && nextest_check.unwrap().status.success() {
    Command::new("cargo")
      .args(&["nextest", "run"])
      .status()
  } else {
    println!("[*] nextest not found, falling back to cargo test");
    Command::new("cargo")
      .args(&["test"])
      .status()
  };
  
  match status {
    Ok(s) if s.success() => println!("[*] Tests passed successfully."),
    _ => {
      eprintln!("[!] Tests failed.");
      exit(1);
    }
  }
}

fn run_bench(bench_name: &str) {
  println!("[*] Running bench: {}", bench_name);
  let mut cmd = Command::new("cargo");
  cmd.arg("bench");
  if !bench_name.is_empty() {
    cmd.args(&["--bench", bench_name]);
  }
  cmd.arg("--");
  cmd.arg("--quiet");
  
  let status = cmd.status().expect("Failed to run cargo bench");
  if !status.success() {
    eprintln!("[!] Benchmark failed.");
    exit(1);
  }
}

fn main() {
  let args: Vec<String> = std::env::args().collect();
  if args.len() < 2 {
    print_usage();
    exit(1);
  }

  let command = &args[1];
  match command.as_str() {
    "mount" | "mr" => {
      mount_rootfs();
    }
    "umount" | "umr" => {
      umount_rootfs();
    }
    "clean-state" => {
      clean_state();
    }
    "test" => {
      run_tests();
    }
    "bench" => {
      let bench_name = args.get(2).map(|s| s.as_str()).unwrap_or("");
      run_bench(bench_name);
    }
    _ => {
      if !command.chars().all(|c| "abniprux".contains(c)) {
        eprintln!("[!] Unknown command or invalid command characters: {}", command);
        print_usage();
        exit(1);
      }
      
      let config: RinbConfig = toml::from_str(&fs::read_to_string("builder.toml").unwrap()).unwrap();
      let profile = config.profile.get("main").unwrap();
      let mut fs_ext4: Option<Ext4Fs> = None;
      for c in command.chars() {
        if c == 'x' {
          continue;
        }
        handle_command(&c.to_string(), profile, &mut fs_ext4, command.contains('x'));
      }
      if let Some(fs) = fs_ext4 {
        let _ = fs.umount();
      }
    }
  }
}
