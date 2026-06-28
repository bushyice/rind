use std::process::Command;

fn get_git_hash() -> String {
  let output = Command::new("git")
    .args(["rev-parse", "--short", "HEAD"])
    .output()
    .expect("git not available");
  String::from_utf8(output.stdout)
    .unwrap_or_default()
    .trim()
    .to_string()
}

fn generate_build_hash() -> String {
  use std::time::{SystemTime, UNIX_EPOCH};
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  format!("{:x}", nanos).chars().take(8).collect()
}

fn main() {
  let git_hash = get_git_hash();
  let build_hash = generate_build_hash();
  println!("cargo:rustc-env=GIT_HASH={git_hash}");
  println!("cargo:rustc-env=BUILD_HASH={build_hash}");
}
