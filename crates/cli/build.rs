// build.rs
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
  println!("cargo:rerun-if-changed=demo/shm.c");

  let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

  let target_dir = out_dir.ancestors().nth(3).unwrap().join("shm");

  let status = Command::new("cc")
    .arg("demo/shm.c")
    .arg("-ldl")
    .arg("-o")
    .arg(&target_dir)
    .status()
    .expect("Failed to execute C compiler");

  if !status.success() {
    panic!("Compilation of shm.c failed.");
  }

  let dummy_rust_out = out_dir.join("shm_dum.rs");
  std::fs::write(&dummy_rust_out, "fn main() {}").unwrap();
}
