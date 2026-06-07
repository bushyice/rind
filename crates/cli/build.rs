// build.rs
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn generate_applets() {
  let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

  let applets_dir = Path::new("src/applets");

  let mut entries = String::new();

  for entry in fs::read_dir(applets_dir).unwrap() {
    let entry = entry.unwrap();
    let path = entry.path();

    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
      continue;
    }

    let stem = path.file_stem().unwrap().to_str().unwrap();

    if stem == "mod"
      || std::env::var_os(format!("CARGO_FEATURE_NO_{}", stem.to_uppercase())).is_some()
    {
      continue;
    }

    entries.push_str(&format!(
      r#"Applet {{
    name: "{stem}",
    entry: crate::applets::{stem}::main,
}},
"#
    ));
  }

  let generated = format!(
    r#"

pub struct Applet {{
    pub name: &'static str,
    pub entry: fn(),
}}

pub static APPLETS: &[Applet] = &[
{entries}
];
"#
  );

  fs::write(out_dir.join("generated_applets.rs"), generated).unwrap();
}

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

  generate_applets();
}
