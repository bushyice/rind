use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rind_ipc::TransportMessage;
use rind_ipc::ser::{deser_from_vec, ser_to_vec};
use rind_ipc::shm::{ShmHeader, ShmRingBuffer};
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;

fn bench_serialization(c: &mut Criterion) {
  let msg = TransportMessage::log("a");

  c.bench_function("serialize_transport_message", |b| {
    b.iter(|| {
      let _ = black_box(&msg).as_bytes();
    })
  });

  let bytes = msg.as_bytes();
  c.bench_function("deserialize_transport_message", |b| {
    b.iter(|| {
      let _: TransportMessage = deser_from_vec(black_box(&bytes), true).unwrap();
    })
  });
}

fn bench_serialization_secondary(c: &mut Criterion) {
  let msg = TransportMessage::log("a");

  c.bench_function("serialize_transport_message_secondary", |b| {
    b.iter(|| {
      let _ = ser_to_vec(black_box(&msg), false);
    })
  });

  let bytes = ser_to_vec(&msg, false);
  c.bench_function("deserialize_transport_message_secondary", |b| {
    b.iter(|| {
      let _: TransportMessage = deser_from_vec(black_box(&bytes), false).unwrap();
    })
  });
}

fn bench_serialization_json(c: &mut Criterion) {
  let msg = TransportMessage::log("a");

  c.bench_function("serialize_transport_message_json", |b| {
    b.iter(|| {
      let _ = serde_json::to_vec(black_box(&msg)).unwrap();
    })
  });

  let bytes = serde_json::to_vec(&msg).unwrap();
  c.bench_function("deserialize_transport_message_json", |b| {
    b.iter(|| {
      let _: TransportMessage = serde_json::from_slice(black_box(&bytes)).unwrap();
    })
  });
}

fn bench_uds_transport(c: &mut Criterion) {
  let (mut tx, mut rx) = UnixStream::pair().unwrap();
  let msg = TransportMessage::log("a");

  c.bench_function("uds_write_read_signed", |b| {
    b.iter(|| {
      msg.write_signed(&mut tx).unwrap();
      let _ = TransportMessage::read_signed(&mut rx).unwrap();
    })
  });
}

fn bench_stdio_sim(c: &mut Criterion) {
  let msg = TransportMessage::log("a");
  let mut buf = Vec::with_capacity(1024);

  c.bench_function("stdio_sim_write_signed", |b| {
    b.iter(|| {
      buf.clear();
      msg.write_signed(&mut buf).unwrap();
      black_box(&buf);
    })
  });
}

fn bench_shm_transport(c: &mut Criterion) {
  const SHM_SIZE: usize = 1024 * 1024;
  let mut buf = vec![0u8; SHM_SIZE];
  let ptr = buf.as_mut_ptr();

  unsafe {
    let h = &mut *(ptr as *mut ShmHeader);
    h.head.store(0, Ordering::Release);
    h.tail.store(0, Ordering::Release);
    h.capacity = SHM_SIZE as u32;
  }

  let ring = unsafe { ShmRingBuffer::new(ptr) };
  let msg = TransportMessage::log("a");
  let data = msg.as_bytes();

  c.bench_function("shm_write_read_raw", |b| {
    b.iter(|| {
      ring.write(black_box(&data));
      let _ = ring.read().unwrap();
    })
  });
}

criterion_group!(
  benches,
  bench_serialization,
  bench_serialization_secondary,
  bench_serialization_json,
  bench_uds_transport,
  bench_stdio_sim,
  bench_shm_transport,
);
criterion_main!(benches);
