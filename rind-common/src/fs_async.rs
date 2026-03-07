use crate::error::report_error;
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

#[derive(Clone, Copy)]
pub enum FileWriteMode {
  Truncate,
  Append,
}

pub struct FileWriteRequest {
  path: PathBuf,
  bytes: Vec<u8>,
  mode: FileWriteMode,
  perms: Option<u32>,
}

static FILE_WRITER: Lazy<Sender<FileWriteRequest>> = Lazy::new(start_writer);

fn start_writer() -> Sender<FileWriteRequest> {
  let (tx, rx) = mpsc::channel::<FileWriteRequest>();
  std::thread::spawn(move || {
    while let Ok(req) = rx.recv() {
      if let Some(parent) = req.path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
      {
        report_error(
          "async writer failed creating parent",
          format!("{}: {err}", parent.display()),
        );
        continue;
      }

      let mut options = std::fs::OpenOptions::new();
      options.create(true).write(true);
      match req.mode {
        FileWriteMode::Truncate => {
          options.truncate(true);
        }
        FileWriteMode::Append => {
          options.append(true);
        }
      }

      #[cfg(unix)]
      {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(mode) = req.perms {
          options.mode(mode);
        }
      }

      match options.open(&req.path) {
        Ok(mut file) => {
          if let Err(err) = std::io::Write::write_all(&mut file, &req.bytes) {
            report_error(
              "async writer write failed",
              format!("{}: {err}", req.path.display()),
            );
          }
        }
        Err(err) => report_error(
          "async writer open failed",
          format!("{}: {err}", req.path.display()),
        ),
      }
    }
  });

  tx
}

pub fn queue_file_write(
  path: impl Into<PathBuf>,
  bytes: Vec<u8>,
  mode: FileWriteMode,
  perms: Option<u32>,
) {
  if let Err(err) = FILE_WRITER.send(FileWriteRequest {
    path: path.into(),
    bytes,
    mode,
    perms,
  }) {
    report_error("async writer queue failed", err);
  }
}

#[cfg(test)]
mod tests {
  use super::{FileWriteMode, queue_file_write};
  use std::time::Duration;

  #[test]
  fn async_writer_truncate_and_append() {
    let mut path = std::env::temp_dir();
    path.push(format!("rind-async-writer-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&path);

    queue_file_write(
      &path,
      b"hello".to_vec(),
      FileWriteMode::Truncate,
      Some(0o600),
    );
    std::thread::sleep(Duration::from_millis(50));
    queue_file_write(
      &path,
      b" world".to_vec(),
      FileWriteMode::Append,
      Some(0o600),
    );
    std::thread::sleep(Duration::from_millis(50));

    let got = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(got, "hello world".to_string());

    let _ = std::fs::remove_file(path);
  }
}
