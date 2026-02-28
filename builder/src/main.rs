// rinb/src/main.rs
use std::{
  collections::HashMap,
  fs::{self, File},
  io::BufReader,
  path::{Path, PathBuf},
  process::{exit, Command},
};

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
struct Profile {
  build_command: Option<String>,
  binary_target: Option<String>,
  binaries: Option<Vec<String>>,
  files: Option<Vec<String>>,   // "src:dst"
  install: Option<Vec<String>>, // URLs of .zst packages
  disk_mode: Option<String>,    // "cpio" or "image"
  linux_image: Option<String>,  // e.g., "bzImage:https://..."
  run_options: Option<Vec<String>>,
  qemu_options: Option<QemuOptions>,
  nodes: Option<Vec<NodeConfig>>,
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
  archive.unpack(target).unwrap();
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

fn prepare_rootfs(profile: &Profile, rootfs: &Path) {
  fs::create_dir_all(rootfs).unwrap();
  fs::create_dir_all(rootfs.join("bin")).unwrap();
  fs::create_dir_all(rootfs.join("etc")).unwrap();

  if let Some(binaries) = &profile.binaries {
    for bin in binaries {
      let src = Path::new(
        &profile
          .binary_target
          .clone()
          .unwrap_or("target/x86_64-unknown-linux-musl/release".to_string()),
      )
      .join(bin);
      let dst = if bin == "init" {
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

  if let Some(files) = &profile.files {
    for mapping in files {
      let parts: Vec<&str> = mapping.splitn(2, ':').collect();
      if parts.len() != 2 {
        eprintln!("Invalid file mapping: {}", mapping);
        continue;
      }
      let src = Path::new(parts[0]);
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
      let size = "512M";
      println!("[*] Creating disk image of size {}", size);
      Command::new("fallocate")
        .args(&["-l", size, output.to_str().unwrap()])
        .status()
        .unwrap();
      Command::new("mkfs.ext4")
        .args(&["-F", output.to_str().unwrap()])
        .status()
        .unwrap();
      let mnt = artifact_path().join("mnt");
      fs::create_dir_all(&mnt).unwrap();
      Command::new("sudo")
        .args(&[
          "mount",
          "-o",
          "loop",
          output.to_str().unwrap(),
          mnt.to_str().unwrap(),
        ])
        .status()
        .unwrap();
      Command::new("sudo")
        .args(&[
          "cp",
          "-r",
          &format!("{}/*", rootfs.display()),
          mnt.to_str().unwrap(),
        ])
        .status()
        .unwrap();
      Command::new("sudo")
        .args(&["umount", mnt.to_str().unwrap()])
        .status()
        .unwrap();
    }
    other => panic!("Unknown disk_mode: {}", other),
  };
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
      cmd.arg("-hda").arg(artifact_path().join("rootfs.img"));
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
    "a" => {
      builder_b(profile);
      builder_i(profile, &rootfs);
      prepare_rootfs(profile, &rootfs);
      builder_n(profile, &rootfs);
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
    eprintln!("Commands: a, b, d, n, i, p, r");
    eprintln!("Examples:");
    eprintln!("build all: a");
    eprintln!("build cargo: b");
    eprintln!("create nodes: n");
    eprintln!("prepare rootfs: p");
    eprintln!("install urls: i");
    eprintln!("build disk: d");
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
