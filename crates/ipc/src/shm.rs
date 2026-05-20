use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
pub struct ShmHeader {
  pub head: AtomicU32,
  pub tail: AtomicU32,
  pub capacity: u32,
}

pub struct ShmRingBuffer {
  ptr: *mut u8,
}

unsafe impl Send for ShmRingBuffer {}
unsafe impl Sync for ShmRingBuffer {}

impl ShmRingBuffer {
  pub unsafe fn new(ptr: *mut u8) -> Self {
    Self { ptr }
  }

  fn header(&self) -> &ShmHeader {
    unsafe { &*(self.ptr as *const ShmHeader) }
  }

  pub fn write(&self, data: &[u8]) -> bool {
    let header = self.header();
    let head = header.head.load(Ordering::Acquire);
    let tail = header.tail.load(Ordering::Acquire);

    let len = data.len() as u32;
    let total_len = len + 4;

    let capacity = header.capacity;
    let data_start = std::mem::size_of::<ShmHeader>() as u32;
    let buffer_size = capacity - data_start;

    let used = if head >= tail {
      head - tail
    } else {
      buffer_size - (tail - head)
    };

    if used + total_len >= buffer_size {
      return false;
    }

    let mut current_head = head;

    let len_bytes = len.to_ne_bytes();
    for i in 0..4 {
      let idx = data_start + (current_head % buffer_size);
      unsafe { *self.ptr.add(idx as usize) = len_bytes[i] };
      current_head += 1;
    }

    for byte in data {
      let idx = data_start + (current_head % buffer_size);
      unsafe { *self.ptr.add(idx as usize) = *byte };
      current_head += 1;
    }

    header.head.store(current_head, Ordering::Release);
    true
  }

  pub fn read(&self) -> Option<Vec<u8>> {
    let header = self.header();
    let head = header.head.load(Ordering::Acquire);
    let mut tail = header.tail.load(Ordering::Acquire);

    if tail == head {
      return None;
    }

    let data_start = std::mem::size_of::<ShmHeader>() as u32;
    let capacity = header.capacity;
    let buffer_size = capacity - data_start;

    let mut len_bytes = [0u8; 4];
    for i in 0..4 {
      let idx = data_start + (tail % buffer_size);
      len_bytes[i] = unsafe { *self.ptr.add(idx as usize) };
      tail += 1;
    }
    let len = u32::from_ne_bytes(len_bytes);

    let mut data = vec![0u8; len as usize];
    for i in 0..len as usize {
      let idx = data_start + (tail % buffer_size);
      data[i] = unsafe { *self.ptr.add(idx as usize) };
      tail += 1;
    }

    header.tail.store(tail, Ordering::Release);
    Some(data)
  }
}
