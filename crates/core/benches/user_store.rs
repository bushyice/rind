use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rind_core::user::UserStore;
use std::fs;
use std::io::Write;
use std::path::Path;

fn temp_dir() -> std::path::PathBuf {
  let dir = std::env::temp_dir().join(format!(
    "rind-user-bench-{}-{}",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos()
  ));
  fs::create_dir_all(&dir).unwrap();
  dir
}

fn write_file(dir: &Path, name: &str, content: &str) {
  let path = dir.join(name);
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).unwrap();
  }
  let mut f = fs::File::create(&path).unwrap();
  f.write_all(content.as_bytes()).unwrap();
}

fn generate_passwd(n: usize) -> String {
  let mut lines = Vec::with_capacity(n);
  for i in 0..n {
    let uid = 1000 + i as u32;
    lines.push(format!(
      "user{i}:x:{uid}:{uid}:User {i}:/home/user{i}:/bin/sh"
    ));
  }
  lines.join("\n")
}

fn generate_shadow(n: usize) -> String {
  let mut lines = Vec::with_capacity(n);
  for i in 0..n {
    lines.push(format!("user{i}:$6$salt$hash:19000:0:99999:7:::"));
  }
  lines.join("\n")
}

fn generate_group(n: usize) -> String {
  let mut lines = Vec::with_capacity(n);
  for i in 0..n {
    let gid = 1000 + i as u32;
    lines.push(format!("user{i}:x:{gid}:"));
  }
  lines.push("wheel:x:10:user0,user1".to_string());
  lines.join("\n")
}

fn bench_user_store_load(c: &mut Criterion) {
  let mut group = c.benchmark_group("user_store_load");

  for count in [10, 100, 1_000, 10_000] {
    let passwd = generate_passwd(count);
    let shadow = generate_shadow(count);
    let grp = generate_group(count);

    group.bench_with_input(
      BenchmarkId::new("from_entries", count),
      &(passwd, shadow, grp),
      |b, (passwd, shadow, grp)| {
        let dir = temp_dir();
        write_file(&dir, "etc/passwd", passwd);
        write_file(&dir, "etc/shadow", shadow);
        write_file(&dir, "etc/group", grp);

        b.iter(|| {
          UserStore::load(
            &dir.join("etc/passwd"),
            &dir.join("etc/shadow"),
            &dir.join("etc/group"),
            &dir.join("etc/rperms"),
          )
          .expect("load should succeed")
        });

        let _ = fs::remove_dir_all(&dir);
      },
    );
  }
  group.finish();
}

fn bench_user_store_parse_passwd(c: &mut Criterion) {
  let mut group = c.benchmark_group("user_store_parse_passwd");

  for count in [100, 1_000, 10_000] {
    let passwd = generate_passwd(count);
    let shadow = generate_shadow(count);
    let grp = generate_group(count);

    group.bench_with_input(BenchmarkId::new("full_load", count), &count, |b, _count| {
      let dir = temp_dir();
      write_file(&dir, "etc/passwd", &passwd);
      write_file(&dir, "etc/shadow", &shadow);
      write_file(&dir, "etc/group", &grp);

      b.iter(|| {
        UserStore::load(
          &dir.join("etc/passwd"),
          &dir.join("etc/shadow"),
          &dir.join("etc/group"),
          &dir.join("etc/rperms"),
        )
        .expect("load should succeed")
      });

      let _ = fs::remove_dir_all(&dir);
    });
  }
  group.finish();
}

fn bench_user_store_lookups(c: &mut Criterion) {
  let dir = temp_dir();
  let count = 10_000;
  write_file(&dir, "etc/passwd", &generate_passwd(count));
  write_file(&dir, "etc/shadow", &generate_shadow(count));
  write_file(&dir, "etc/group", &generate_group(count));

  let store = UserStore::load(
    &dir.join("etc/passwd"),
    &dir.join("etc/shadow"),
    &dir.join("etc/group"),
    &dir.join("etc/rperms"),
  )
  .expect("load should succeed");

  let mut group = c.benchmark_group("user_store_lookups");

  group.bench_function("lookup_by_name", |b| {
    b.iter(|| {
      for i in 0..1000 {
        store.lookup_by_name(&format!("user{}", i % count));
      }
    })
  });

  group.bench_function("lookup_by_uid", |b| {
    b.iter(|| {
      for i in 0..1000 {
        store.lookup_by_uid(1000 + (i as u32 % count as u32));
      }
    })
  });

  group.bench_function("shadow_for", |b| {
    b.iter(|| {
      for i in 0..1000 {
        store.shadow_for(&format!("user{}", i % count));
      }
    })
  });

  group.bench_function("groups_for", |b| {
    let user = store.lookup_by_name("user0").unwrap();
    b.iter(|| {
      for _ in 0..1000 {
        store.groups_for(user);
      }
    })
  });

  group.finish();
  let _ = fs::remove_dir_all(&dir);
}

criterion_group!(
  benches,
  bench_user_store_load,
  bench_user_store_parse_passwd,
  bench_user_store_lookups
);
criterion_main!(benches);
