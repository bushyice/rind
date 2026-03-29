/// mostly by: GPT-5 Mini
use std::{
  collections::HashMap,
  fs::{self, File},
  io::BufReader,
  path::{Path, PathBuf},
  process::{exit, Command},
};

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;

use ext4_lwext4::{mkfs, Ext4Fs, FileBlockDevice, MkfsOptions, OpenFlags};
use fs_extra::dir::CopyOptions;
use reqwest::blocking::get;
use serde::Deserialize;
use tar::Archive;
use zstd::stream::read::Decoder;

use nix::sys::stat::{makedev, mknod, Mode, SFlag};

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
}

#[derive(Debug, Deserialize)]
struct Profile {
  build_command: Option<String>,
  binary_target: Option<String>,
  binaries: Option<Vec<String>>,
  files: Option<Vec<String>>, // "src:dst"
  libs: Option<Vec<String>>,
  install: Option<Vec<String>>, // URLs of .zst packages
  disk_mode: Option<String>,    // "cpio" or "image"
  linux_image: Option<String>,  // e.g., "bzImage:https://..."
  run_options: Option<Vec<String>>,
  qemu_options: Option<QemuOptions>,
  nodes: Option<Vec<NodeConfig>>,
  busybox_url: Option<String>,
  busybox_applets: Option<Vec<String>>,
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

  println!("[*] Downloading: {}", url);
  fs::create_dir_all(artifact_path()).unwrap();
  let resp = get(url).unwrap().bytes().unwrap();
  fs::write(&path, &resp).unwrap();
  path
}

fn extract_zst(zst_path: &Path, target: &Path) {
  println!(
    "[*] Extracting {} into {}",
    zst_path.display(),
    target.display()
  );
  let file = File::open(zst_path).unwrap();
  let reader = BufReader::new(file);
  let mut decoder = Decoder::new(reader).unwrap();
  let mut archive = Archive::new(&mut decoder);
  let _ = archive.unpack(target);
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
  fs::create_dir_all(rootfs.join("bin")).unwrap();
  fs::create_dir_all(rootfs.join("etc")).unwrap();
  fs::create_dir_all(rootfs.join("usr")).unwrap();
  fs::create_dir_all(rootfs.join("var")).unwrap();

  if let Some(binaries) = &profile.binaries {
    for bin in binaries {
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(bin);
      let dst = if bin == "initd" {
        rootfs.join(bin)
      } else {
        rootfs.join("bin").join(bin)
      };
      if !dst.exists()
        || fs::metadata(&src).unwrap().modified().unwrap()
          > fs::metadata(&dst).unwrap().modified().unwrap()
      {
        println!("[*] Updating binary: {}", bin);
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
  if let Some(installs) = &profile.install {
    for url in installs {
      let archive = cached_download(url, None);
      extract_zst(&archive, rootfs);
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

    let bin_dir = rootfs.join("bin");
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

  let mut passwd = String::new();
  let mut shadow = String::new();
  let mut group_map: HashMap<String, (u32, Vec<String>)> = HashMap::new();

  passwd.push_str("root:x:0:0:root:/root:/bin/sh\n");
  shadow.push_str("root:*:19000:0:99999:7:::\n");
  group_map.insert("root".into(), (0, vec!["root".into()]));
  group_map.insert("wheel".into(), (10, vec!["root".into()]));

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

fn builder_d(profile: &Profile, rootfs: &Path) {
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
        fs::remove_file(&output).unwrap();
      }

      let size_bytes = 1024 * 1024 * 1024;
      println!("[*] Creating ext4 disk image of size {} bytes", size_bytes);

      let device =
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
      let device = FileBlockDevice::open(&output).expect("Failed to open image");
      let mut fs = Ext4Fs::mount(device, false).expect("Failed to mount device");

      copy_into_ext4(&mut fs, rootfs, rootfs).expect("Failed to copy rootfs recursively");

      println!("[*] Disk image created at {}", output.display());
    }
    other => panic!("Unknown disk_mode: {}", other),
  };
}

fn copy_into_ext4(fs: &mut Ext4Fs, rootfs: &Path, current: &Path) -> std::io::Result<()> {
  for entry in fs::read_dir(current)? {
    let entry = entry?;
    let path = entry.path();

    let rel_path = path.strip_prefix(rootfs).unwrap();
    let target_path = format!("/{}", rel_path.display());

    println!("Copying: {:?}", path);

    let meta = std::fs::metadata(&path)?;
    let mode = meta.mode() & 0o7777;
    let uid = meta.uid();
    let gid = meta.gid();

    if path.is_dir() {
      fs.mkdir(&target_path, mode)
        .expect("Failed to create directory");
      fs.set_owner(&target_path, uid, gid)
        .expect("Failed to set directory owner");
      fs.set_permissions(&target_path, mode)
        .expect("Failed to set directory mode");

      copy_into_ext4(fs, rootfs, &path)?;
    } else {
      let bytes = fs::read(&path)?;

      let mut file = fs
        .open(&target_path, OpenFlags::CREATE | OpenFlags::WRITE)
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

fn handle_command(c: &str, profile: &Profile, rootfs: &Path) {
  match c {
    "b" => builder_b(profile),
    "n" => builder_n(profile, &rootfs),
    "i" => builder_i(profile, &rootfs),
    "d" => builder_d(profile, &rootfs),
    "p" => prepare_rootfs(profile, &rootfs),
    "u" => builder_u(profile, &rootfs),
    "a" => {
      builder_b(profile);
      builder_i(profile, &rootfs);
      prepare_rootfs(profile, &rootfs);
      builder_n(profile, &rootfs);
      builder_u(profile, &rootfs);
      builder_d(profile, &rootfs);
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

  for c in args[1].chars() {
    handle_command(&c.to_string(), profile, &rootfs)
  }
}
