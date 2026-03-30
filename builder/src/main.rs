/// mostly by: GPT-5 Mini
use std::{
  collections::BTreeSet,
  collections::HashMap,
  fs,
  io::ErrorKind,
  path::{Path, PathBuf},
  process::{exit, Command},
};

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;

use ext4_lwext4::{mkfs, Ext4Fs, FileBlockDevice, MkfsOptions, OpenFlags};
use fs_extra::dir::CopyOptions;
use nix::sys::stat::{makedev, mknod, Mode, SFlag};
use reqwest::blocking::get;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RinbConfig {
  profile: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize)]
struct UserConfig {
  username: String,
  password: Option<String>,
  uid: u32,
  gid: u32,
  home: String,
  shell: String,
  groups: Option<Vec<String>>,
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
  bins_dir: Option<String>, // "/bin" or "/usr/bin"
  binaries: Option<Vec<String>>,
  files: Option<Vec<String>>, // "src:dst"
  libs: Option<Vec<String>>,
  install: Option<Vec<String>>, // "url" or "alias:url"
  disk_mode: Option<String>,    // "cpio" or "image"
  linux_image: Option<String>,  // e.g., "bzImage:https://..."
  run_options: Option<Vec<String>>,
  qemu_options: Option<QemuOptions>,
  nodes: Option<Vec<NodeConfig>>,
  busybox_url: Option<String>,
  busybox_applets: Option<Vec<String>>,
  symlinks: Option<Vec<String>>, // "target:link"
  ldd_bins: Option<Vec<String>>, // absolute binary paths in rootfs
  #[serde(alias = "root_envs")]
  root_env: Option<EnvConfig>,
  user: Option<Vec<UserConfig>>,
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
    let v = map.get(k).unwrap();
    out.push_str(&format!("{k}={v}\n"));
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
    println!("[*] Using cached: {}", filename);
    return path;
  }

  println!("[*] Downloading: {} into {}", url, filename);
  fs::create_dir_all(artifact_path()).unwrap();
  let resp = get(url).unwrap().bytes().unwrap();
  fs::write(&path, &resp).unwrap();
  path
}

fn parse_install_entry(entry: &str) -> (String, String) {
  if entry.starts_with("http://") || entry.starts_with("https://") {
    let alias = entry
      .split('/')
      .last()
      .unwrap_or(entry)
      .trim_end_matches(".tar.gz")
      .trim_end_matches(".tgz")
      .trim_end_matches(".tar.xz")
      .trim_end_matches(".tar.zst")
      .trim_end_matches(".pkg.tar.zst")
      .trim_end_matches(".pkg.tar.xz")
      .trim_end_matches(".pkg.tar.gz")
      .trim_end_matches(".tar")
      .to_string();
    return (alias, entry.to_string());
  }

  let Some((alias, url)) = entry.split_once(':') else {
    panic!("Invalid install entry. Use url or alias:url. Got: {entry}");
  };
  if !(url.starts_with("http://") || url.starts_with("https://")) {
    panic!("Invalid install url in entry: {entry}");
  }
  (alias.to_string(), url.to_string())
}

fn remove_existing(path: &Path) -> std::io::Result<()> {
  match fs::symlink_metadata(path) {
    Ok(meta) => {
      if meta.file_type().is_dir() {
        fs::remove_dir_all(path)
      } else {
        fs::remove_file(path)
      }
    }
    Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
    Err(e) => Err(e),
  }
}

fn extract_from_archive(archive: &Path, member: &str, dst: &Path) {
  let member = member.trim_start_matches('/');
  let unpack_root = artifact_path().join("unpack");
  let archive_key = archive
    .file_name()
    .unwrap()
    .to_string_lossy()
    .replace('/', "_");
  let unpack_dir = unpack_root.join(&archive_key);
  let stamp = unpack_dir.join(".unpacked_from");

  let archive_id = format!(
    "{}:{}",
    archive.display(),
    fs::metadata(archive)
      .and_then(|m| m.modified())
      .map(|m| format!("{m:?}"))
      .unwrap_or_default()
  );

  let mut needs_unpack = true;
  if unpack_dir.exists() {
    if let Ok(prev) = fs::read_to_string(&stamp) {
      if prev == archive_id {
        needs_unpack = false;
      }
    }
  }

  if needs_unpack {
    remove_existing(&unpack_dir).ok();
    fs::create_dir_all(&unpack_dir).unwrap();

    let status = Command::new("tar")
      .arg("-xf")
      .arg(archive)
      .arg("-C")
      .arg(&unpack_dir)
      .status()
      .unwrap();
    if !status.success() {
      panic!("Failed to unpack archive {}", archive.display());
    }

    fs::write(&stamp, archive_id).unwrap();
  }

  let extracted = unpack_dir.join(member);
  if !extracted.exists() {
    panic!(
      "Archive member '{member}' not found in unpacked archive {}",
      archive.display()
    );
  }

  if let Some(parent) = dst.parent() {
    fs::create_dir_all(parent).unwrap();
  }
  remove_existing(dst).ok();

  // Preserve mode/timestamps/symlinks/hardlinks while avoiding ownership failures as non-root.
  let status = Command::new("cp")
    .arg("-a")
    .arg("--no-preserve=ownership")
    .arg("-T")
    .arg(&extracted)
    .arg(dst)
    .status()
    .unwrap();
  if !status.success() {
    panic!(
      "Failed to copy extracted member '{}' to {}",
      member,
      dst.display()
    );
  }
}

fn apply_symlinks(profile: &Profile, rootfs: &Path) {
  let Some(symlinks) = &profile.symlinks else {
    return;
  };

  for mapping in symlinks {
    let parts: Vec<&str> = mapping.splitn(2, ':').collect();
    if parts.len() != 2 {
      eprintln!("Invalid symlink mapping: {}", mapping);
      continue;
    }

    let target = parts[0]; //rootfs.join(parts[0].trim_start_matches('/'));
    let link = rootfs.join(parts[1].trim_start_matches('/'));
    if let Some(parent) = link.parent() {
      fs::create_dir_all(parent).unwrap();
    }

    if let Err(e) = remove_existing(&link) {
      eprintln!("Failed to reset existing path {}: {e}", link.display());
      continue;
    }

    std::os::unix::fs::symlink(&target, &link).unwrap();
    println!("[*] Symlink: {} -> {}", link.display(), target);
  }
}

fn parse_ldd_output(line: &str) -> Option<String> {
  let trimmed = line.trim();
  if trimmed.is_empty()
    || trimmed.starts_with("linux-vdso.so")
    || trimmed.contains("statically linked")
    || trimmed.contains("not a dynamic executable")
    || trimmed.contains("=> not found")
  {
    return None;
  }

  if let Some((_left, right)) = trimmed.split_once("=>") {
    let rhs = right.trim();
    if let Some(path) = rhs.split_whitespace().next() {
      if path.starts_with('/') {
        return Some(path.to_string());
      }
    }
    return None;
  }

  if let Some(path) = trimmed.split_whitespace().next() {
    if path.starts_with('/') {
      return Some(path.to_string());
    }
  }

  None
}

fn copy_host_path_into_rootfs(src: &Path, rootfs: &Path) {
  if !src.is_absolute() {
    return;
  }
  let meta = match fs::symlink_metadata(src) {
    Ok(meta) => meta,
    Err(_) => return,
  };
  let dst = rootfs.join(src.strip_prefix("/").unwrap());
  if let Some(parent) = dst.parent() {
    fs::create_dir_all(parent).unwrap();
  }

  if meta.file_type().is_symlink() {
    let target = fs::read_link(src).unwrap();
    remove_existing(&dst).ok();
    std::os::unix::fs::symlink(&target, &dst).unwrap();
    let resolved = if target.is_absolute() {
      target
    } else {
      src.parent().unwrap().join(target)
    };
    copy_host_path_into_rootfs(&resolved, rootfs);
    return;
  }

  let should_copy = !dst.exists()
    || fs::metadata(src).unwrap().modified().unwrap()
      > fs::metadata(&dst).unwrap().modified().unwrap();
  if should_copy {
    fs::copy(src, &dst).unwrap();
    let mut perms = fs::metadata(src).unwrap().permissions();
    perms.set_mode(fs::metadata(src).unwrap().permissions().mode());
    fs::set_permissions(&dst, perms).unwrap();
    println!("[*] Copied ldd dep: {} -> {}", src.display(), dst.display());
  }
}

fn copy_ldd_dependencies(profile: &Profile, rootfs: &Path) {
  let Some(bins) = &profile.ldd_bins else {
    return;
  };

  let mut deps = BTreeSet::new();
  for bin in bins {
    let bin_path = rootfs.join(bin.trim_start_matches('/'));
    if !bin_path.exists() {
      eprintln!("[!] ldd target missing in rootfs: {}", bin_path.display());
      continue;
    }

    let output = Command::new("ldd").arg(&bin_path).output();
    let output = match output {
      Ok(output) => output,
      Err(e) => {
        eprintln!("[!] Failed to run ldd on {}: {e}", bin_path.display());
        continue;
      }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    for line in combined.lines() {
      if let Some(path) = parse_ldd_output(line) {
        deps.insert(path);
      }
    }
  }

  for dep in deps {
    copy_host_path_into_rootfs(Path::new(&dep), rootfs);
  }
}

fn builder_b(profile: &Profile) {
  if let Some(cmd) = &profile.build_command {
    println!("[*] Running build command: {}", cmd);
    let status = if cfg!(target_os = "windows") {
      Command::new("cmd").args(&["/C", cmd]).status().unwrap()
    } else {
      Command::new("sh").args(&["-c", cmd]).status().unwrap()
    };
    if !status.success() {
      eprintln!("Build command failed");
      exit(1);
    }
  }
}

/// mostly by me
fn prepare_rootfs(profile: &Profile, rootfs: &Path) {
  fs::create_dir_all(rootfs).unwrap();
  fs::create_dir_all(rootfs.join("etc")).unwrap();
  fs::create_dir_all(rootfs.join("usr")).unwrap();
  fs::create_dir_all(rootfs.join(configured_bins_dir(profile).trim_start_matches('/'))).unwrap();
  fs::create_dir_all(rootfs.join("var")).unwrap();

  if let Some(binaries) = &profile.binaries {
    for bin in binaries {
      let (bin_name, bin_dst_override): (&str, Option<String>) =
        if let Some((left, right)) = bin.split_once(':') {
          let left_trim = left.trim_start_matches('/');
          if left_trim == "bin" || left_trim == "usr/bin" {
            (right, Some(format!("/{left_trim}/{right}")))
          } else {
            (left, Some(right.to_string()))
          }
        } else {
          (bin.as_str(), None)
        };
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(bin_name);
      let dst = if let Some(abs_path) = bin_dst_override {
        let abs_path = if abs_path.starts_with('/') {
          abs_path
        } else {
          format!("/{abs_path}")
        };
        rootfs.join(abs_path.trim_start_matches('/'))
      } else if bin_name == "initd" {
        rootfs.join(bin_name)
      } else {
        rootfs
          .join(configured_bins_dir(profile).trim_start_matches('/'))
          .join(bin_name)
      };
      if !dst.exists()
        || fs::metadata(&src).unwrap().modified().unwrap()
          > fs::metadata(&dst).unwrap().modified().unwrap()
      {
        println!("[*] Updating binary: {} -> {}", bin_name, dst.display());
        if let Some(parent) = dst.parent() {
          fs::create_dir_all(parent).unwrap();
        }
        fs::copy(&src, &dst).unwrap();
      }
    }
  }

  if let Some(libs) = &profile.libs {
    let incl_dst = rootfs.join("usr/include");
    let lib_dst = rootfs.join("usr/lib");
    fs::create_dir_all(&incl_dst).unwrap();
    fs::create_dir_all(&lib_dst).unwrap();

    for lib in libs {
      let parts: Vec<&str> = lib.splitn(2, ':').collect();
      if parts.len() != 2 {
        eprintln!("Invalid library mapping: {}", lib);
        continue;
      }
      let libname = format!("lib{}.a", parts[0].replace("-", "_"));
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(libname.clone());
      let dst = lib_dst.join(libname);

      if !dst.exists()
        || fs::metadata(&src).unwrap().modified().unwrap()
          > fs::metadata(&dst).unwrap().modified().unwrap()
      {
        println!("[*] Updating library: {}", lib);

        fs::copy(&src, &dst).unwrap();

        cbindgen::Builder::new()
          .with_crate(parts[0])
          .generate()
          .expect("Unable to generate bindings")
          .write_to_file(incl_dst.join(format!("{}.h", parts[1])));
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
        eprintln!("Invalid file mapping: {}", mapping);
        continue;
      }
      let src = Path::new(parts[0]);
      if !src.exists() {
        eprintln!("File does not exist: {}", mapping);
        continue;
      }
      let dst = rootfs.join(parts[1].trim_start_matches('/'));

      if !dst.exists()
        || fs::metadata(&src).unwrap().modified().unwrap()
          > fs::metadata(&dst).unwrap().modified().unwrap()
      {
        println!("[*] Updating file: {} -> {}", src.display(), dst.display());
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        if dst.exists() {
          fs::remove_file(&dst).ok();
        }
        if src.is_dir() {
          fs_extra::dir::copy(
            src,
            dst.parent().unwrap(),
            &CopyOptions::new().overwrite(true).copy_inside(true),
          )
          .unwrap();
        } else {
          fs::copy(src, &dst).unwrap();
        }
      }
    }
  }

  apply_symlinks(profile, rootfs);
  copy_ldd_dependencies(profile, rootfs);
}

fn builder_n(profile: &Profile, rootfs: &Path) {
  if let Some(nodes) = &profile.nodes {
    println!("[*] Creating device nodes...");
    for node in nodes {
      let full_path = rootfs.join(node.path.trim_start_matches('/'));
      if full_path.exists() {
        println!("[*] Node exists, skipping: {}", node.path);
        continue;
      }
      fs::create_dir_all(full_path.parent().unwrap()).unwrap();
      mknod(
        &full_path,
        SFlag::from_bits_truncate(match node.node_type.as_str() {
          "c" => SFlag::S_IFCHR.bits(),
          "b" => SFlag::S_IFBLK.bits(),
          _ => panic!("Unknown node type"),
        }),
        Mode::from_bits_truncate(node.mode),
        makedev(node.major, node.minor),
      )
      .unwrap();
    }
  }
}

fn builder_i(profile: &Profile, rootfs: &Path) {
  let mut archive_cache: HashMap<String, PathBuf> = HashMap::new();

  if let Some(installs) = &profile.install {
    for entry in installs {
      let (alias, url) = parse_install_entry(entry);
      let source_name = url.split('/').last().unwrap_or(&alias);
      let cache_name = format!("{alias}__{source_name}");
      let archive = cached_download(&url, Some(&cache_name));
      archive_cache.insert(alias, archive);
    }
  }

  if let Some(files) = &profile.files {
    for mapping in files {
      if !mapping.starts_with('@') {
        continue;
      }

      let parts: Vec<&str> = mapping.splitn(3, ':').collect();
      if parts.len() != 3 {
        eprintln!(
          "Invalid archive file mapping: {} (expected @alias:member:/dst)",
          mapping
        );
        continue;
      }

      let alias = parts[0].trim_start_matches('@');
      let member = parts[1];
      let dst = rootfs.join(parts[2].trim_start_matches('/'));
      let Some(archive) = archive_cache.get(alias) else {
        eprintln!(
          "Archive alias not found for mapping '{}': {}",
          alias, mapping
        );
        continue;
      };
      println!(
        "[*] Extracting {} from {} -> {}",
        member,
        archive.display(),
        dst.display()
      );
      extract_from_archive(archive, member, &dst);
    }
  }

  if let Some(kernel_url) = &profile.linux_image {
    let (name, url) = kernel_url
      .split_once(":")
      .expect("Invalid linux_image format. Expected name:url");
    cached_download(url, Some(name));
  }

  if let Some(url) = &profile.busybox_url {
    let bb_path = cached_download(url, None);

    let bin_dir = rootfs.join(configured_bins_dir(profile).trim_start_matches('/'));
    fs::create_dir_all(&bin_dir).unwrap();

    let busybox_dst = bin_dir.join("busybox");
    fs::copy(&bb_path, &busybox_dst).unwrap();

    let mut perms = fs::metadata(&busybox_dst).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&busybox_dst, perms).unwrap();

    let applets = profile.busybox_applets.clone().unwrap_or(vec![
      "sh".into(),
      "ls".into(),
      "cp".into(),
      "mkdir".into(),
      "echo".into(),
      "cat".into(),
      "rm".into(),
      "ln".into(),
    ]);

    for app in &applets {
      let link_path = bin_dir.join(app);
      if !link_path.exists() {
        std::os::unix::fs::symlink("busybox", link_path).unwrap();
      }
    }

    println!(
      "[*] BusyBox installed at /bin/busybox with applets: {:?}",
      applets
    );
  }
}

fn builder_u(profile: &Profile, rootfs: &Path) {
  let etc_dir = rootfs.join("etc");
  fs::create_dir_all(&etc_dir).unwrap();
  let rind_env_dir = etc_dir.join("env");
  let users_env_dir = rind_env_dir.join("users");
  fs::create_dir_all(&users_env_dir).unwrap();

  let mut passwd = String::new();
  let mut shadow = String::new();
  let mut group_map: HashMap<String, (u32, Vec<String>)> = HashMap::new();

  passwd.push_str("root:x:0:0:root:/root:/bin/sh\n");
  shadow.push_str("root:*:19000:0:99999:7:::\n");
  group_map.insert("root".into(), (0, vec!["root".into()]));
  group_map.insert("wheel".into(), (10, vec!["root".into()]));

  if let Some(root_env) = &profile.root_env {
    let root_env_map = root_env.to_map();
    fs::write(
      rind_env_dir.join("root.env"),
      render_env_lines(&root_env_map),
    )
    .unwrap();
  }

  if let Some(users) = &profile.user {
    println!("[*] Generating user databases...");

    for user in users {
      let home_path = rootfs.join(user.home.trim_start_matches('/'));
      fs::create_dir_all(&home_path).unwrap();

      unsafe {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let path_c = CString::new(home_path.as_os_str().as_bytes()).unwrap();

        if libc::chown(path_c.as_ptr(), user.uid, user.gid) != 0 {
          panic!("failed to chown {}", home_path.display());
        }
      }

      passwd.push_str(&format!(
        "{}:x:{}:{}:Linux User:{}:{}\n",
        user.username, user.uid, user.gid, user.home, user.shell
      ));

      let hash = if let Some(pass) = &user.password {
        sha_crypt::sha512_simple(pass, &sha_crypt::Sha512Params::new(5000).unwrap())
          .unwrap_or("*".to_string())
      } else {
        "*".to_string()
      };

      shadow.push_str(&format!("{}:{}:19000:0:99999:7:::\n", user.username, hash));

      let primary_group = user.username.clone();
      group_map
        .entry(primary_group)
        .or_insert((user.gid, Vec::new()))
        .1
        .push(user.username.clone());

      if let Some(groups) = &user.groups {
        for g in groups {
          group_map
            .entry(g.clone())
            .or_insert((1000, Vec::new()))
            .1
            .push(user.username.clone());
        }
      }

      if let Some(env) = &user.env {
        let env_map = env.to_map();
        fs::write(
          users_env_dir.join(format!("{}.env", user.username)),
          render_env_lines(&env_map),
        )
        .unwrap();
      }
    }
  }

  let mut group_str = String::new();
  for (name, (gid, members)) in group_map {
    group_str.push_str(&format!("{}:x:{}:{}\n", name, gid, members.join(",")));
  }

  fs::write(etc_dir.join("passwd"), passwd).unwrap();
  fs::write(etc_dir.join("shadow"), shadow).unwrap();
  fs::write(etc_dir.join("group"), group_str).unwrap();

  let mut perms = fs::metadata(etc_dir.join("shadow")).unwrap().permissions();
  perms.set_mode(0o600);
  fs::set_permissions(etc_dir.join("shadow"), perms).unwrap();
}

fn builder_d(profile: &Profile, rootfs: &Path, no_overwrite: bool) {
  match profile.disk_mode.as_deref().unwrap_or("cpio") {
    "cpio" => {
      let output = artifact_path().join("rootfs.cpio.gz");
      if output.exists() {
        println!("[*] Updating existing initramfs");
        fs::remove_file(&output).unwrap();
      }
      println!("[*] Generating initramfs from: {}", rootfs.display());
      let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
          "cd {} && find . | cpio -H newc -o | gzip > ../../{}",
          rootfs.display(),
          output.display()
        ))
        .status()
        .unwrap();
      if !status.success() {
        eprintln!("Failed to generate initramfs");
        exit(1);
      }
    }
    "image" => {
      let output = artifact_path().join("rootfs.img");
      if output.exists() {
        println!("[*] Updating existing disk image");
        if !no_overwrite {
          fs::remove_file(&output).unwrap();
        }
      }

      if !no_overwrite {
        let size_bytes = 1024 * 1024 * 1024;
        println!("[*] Creating ext4 disk image of size {} bytes", size_bytes);

        let _ =
          FileBlockDevice::create(&output, size_bytes).expect("Failed to create block device");

        Command::new("mkfs.ext4")
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
          .unwrap();
      }

      let mut fs = {
        // Auto check (because there's no graceful exit)
        let _ = Command::new("e2fsck")
          .arg("-p")
          .arg("-f")
          .arg(&output)
          .status();

        let device = FileBlockDevice::open(&output).expect("Failed to open image");
        let fs = Ext4Fs::mount(device, false).expect("Failed to mount device");

        if fs.is_read_only() {
          eprintln!(
            "[!] ext4 image mounted read-only. Trying repair with e2fsck: {}",
            output.display()
          );
          let _ = fs.umount();

          let status = Command::new("e2fsck")
            .arg("-p")
            .arg("-f")
            .arg(&output)
            .status();

          match status {
            Ok(s) if s.success() => {
              let device = FileBlockDevice::open(&output).expect("Failed to reopen image");
              let remounted = Ext4Fs::mount(device, false).expect("Failed to remount device");
              if remounted.is_read_only() {
                panic!(
                  "image still mounted read-only after fsck: {}. recreate the image with `builder d` (without x).",
                  output.display()
                );
              }
              remounted
            }
            Ok(s) => {
              panic!("e2fsck failed with status {} for {}", s, output.display());
            }
            Err(e) => {
              panic!(
                "failed to run e2fsck for {}: {} (install e2fsprogs)",
                output.display(),
                e
              );
            }
          }
        } else {
          fs
        }
      };

      copy_into_ext4(&mut fs, rootfs, rootfs).expect("Failed to copy rootfs recursively");

      println!("[*] Disk image created at {}", output.display());
    }
    other => panic!("Unknown disk_mode: {}", other),
  };
}

fn copy_into_ext4(fs: &mut Ext4Fs, rootfs: &Path, current: &Path) -> std::io::Result<()> {
  let entries = match fs::read_dir(current) {
    Ok(e) => e,
    Err(e) => {
      eprintln!("Failed to read {current:?}: {e}");
      return Err(e);
    }
  };
  for entry in entries {
    let entry = entry?;
    let path = entry.path();

    let rel_path = path.strip_prefix(rootfs).unwrap();
    let target_path = format!("/{}", rel_path.display());

    let Ok(meta) = std::fs::symlink_metadata(&path) else {
      println!("Skipping: {:?}", path);
      continue;
    };
    let file_type = meta.file_type();
    let mode = meta.mode() & 0o7777;
    let uid = meta.uid();
    let gid = meta.gid();

    if file_type.is_symlink() {
      let link_target = std::fs::read_link(&path)?;
      let link_target = link_target.to_string_lossy().to_string();
      fs.symlink(&link_target, &target_path)
        .expect("Failed to create symlink");
      fs.set_owner(&target_path, uid, gid)
        .expect("Failed to set symlink owner");
    } else if file_type.is_dir() {
      if !fs.exists(&target_path) {
        fs.mkdir(&target_path, mode)
          .expect("Failed to create directory");
        fs.set_owner(&target_path, uid, gid)
          .expect("Failed to set directory owner");
        fs.set_permissions(&target_path, mode)
          .expect("Failed to set directory mode");
      }

      copy_into_ext4(fs, rootfs, &path)?;
    } else {
      if fs.exists(&target_path) {
        let t = fs::metadata(&path)
          .unwrap()
          .modified()
          .unwrap()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap()
          .as_secs();
        if t <= fs.metadata(&target_path).unwrap().mtime {
          continue;
        }
        println!("Updating file: {:?}", target_path);
      }

      let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
          let original_mode = mode;
          let tmp_mode = original_mode | 0o400;

          let mut perms = std::fs::metadata(&path)?.permissions();
          perms.set_mode(tmp_mode);
          std::fs::set_permissions(&path, perms)?;

          let read_res = fs::read(&path);

          let mut restore = std::fs::metadata(&path)?.permissions();
          restore.set_mode(original_mode);
          if let Err(err) = std::fs::set_permissions(&path, restore) {
            eprintln!(
              "[!] Warning: failed to restore mode on {}: {err}",
              path.display()
            );
          }

          read_res?
        }
        Err(e) => {
          eprintln!("Failed to read {path:?}: {e}");
          return Err(e);
        }
      };

      let mut file = fs
        .open(
          &target_path,
          OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .expect("Failed to open file");

      file.write_all(&bytes).expect("Failed to write file");
      fs.set_owner(&target_path, uid, gid)
        .expect("Failed to set file owner");
      fs.set_permissions(&target_path, mode)
        .expect("Failed to set file mode");
    }
  }

  Ok(())
}

fn run(profile: &Profile) {
  let kernel_path = if let Some(k) = &profile.linux_image {
    let (name, _url) = k
      .split_once(":")
      .expect("Invalid linux_image format. Expected name:url");
    artifact_path().join(name)
  } else {
    panic!("No kernel specified");
  };

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
  cmd.arg("-kernel").arg(kernel_path);

  match profile.disk_mode.as_deref().unwrap_or("cpio") {
    "cpio" => {
      cmd
        .arg("-initrd")
        .arg(artifact_path().join("rootfs.cpio.gz"));
    }
    "image" => {
      cmd.arg("-drive").arg(format!(
        "file={},format=raw,if=virtio",
        artifact_path().join("rootfs.img").display()
      ));
      // cmd.arg("-hda").arg(artifact_path().join("rootfs.img"));
    }
    _ => panic!("Unknown disk_mode"),
  }

  if let Some(options) = &profile.run_options {
    cmd.arg("-append").arg(options.join(" "));
  }

  if let Some(args) = &qemu_options.args {
    cmd.args(args);
  }

  println!("[*] Launching QEMU...");

  let status = cmd.status().unwrap();
  if !status.success() {
    eprintln!("QEMU failed");
    exit(1);
  }
}

fn handle_command(c: &str, profile: &Profile, rootfs: &Path, no_overwrite: bool) {
  match c {
    "b" => builder_b(profile),
    "n" => builder_n(profile, &rootfs),
    "i" => builder_i(profile, &rootfs),
    "d" => builder_d(profile, &rootfs, no_overwrite),
    "p" => prepare_rootfs(profile, &rootfs),
    "u" => builder_u(profile, &rootfs),
    "a" => {
      builder_b(profile);
      builder_i(profile, &rootfs);
      prepare_rootfs(profile, &rootfs);
      builder_n(profile, &rootfs);
      builder_u(profile, &rootfs);
      builder_d(profile, &rootfs, no_overwrite);
    }
    "r" => {
      run(profile);
    }
    other => {
      eprintln!("Unknown builder command: {}", other);
      exit(1);
    }
  }
}

fn main() {
  let args: Vec<String> = std::env::args().collect();
  if args.len() < 2 {
    eprintln!("Usage: builder <builder_command>");
    eprintln!("Commands: a, b, d, n, i, p, r, u");
    eprintln!("Examples:");
    eprintln!("build all: a");
    eprintln!("build cargo: b");
    eprintln!("create nodes: n");
    eprintln!("prepare rootfs: p");
    eprintln!("install urls: i");
    eprintln!("build disk: d");
    eprintln!("make users: u");
    eprintln!("run: r");
    eprintln!(
      "you can use multiple commands, for example this builds cargo, prepares disk and runs: bdr"
    );
    eprintln!("you can even do this lol: rind");
    exit(1);
  }

  let config_data = fs::read_to_string("builder.toml").unwrap();
  let config: RinbConfig = toml::from_str(&config_data).unwrap();
  let profile = config.profile.get("main").unwrap();
  let rootfs = artifact_path().join("rootfs");
  fs::create_dir_all(&rootfs).unwrap();
  let mut no_overwrite = false;

  if args[1].contains("x") {
    no_overwrite = true;
  }

  for c in args[1].chars() {
    if c == 'x' {
      continue;
    }
    handle_command(&c.to_string(), profile, &rootfs, no_overwrite)
  }
}
