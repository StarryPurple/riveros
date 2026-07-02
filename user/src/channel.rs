use super::*;
/// [head (total read bytes): usize][tail (total written bytes): usize][ring buffer: u8[capacity]]
pub struct Channel {
    buf: *mut u8,
    capacity: usize,
}

impl Channel {
    pub fn new(buf: *mut u8, buf_size: usize) -> Self {
      unsafe {
        core::ptr::write_volatile(buf as *mut usize, 0);
        core::ptr::write_volatile(buf.add(8) as *mut usize, 0);
      }
      Self {
        buf,
        capacity: buf_size - 16,
      }
    }
    /// Sends the entire data. All or nothing. Blocks until the data is sent.
    pub fn send(&self, data: &[u8]) {
      while !self.try_send(data) {
        yield_();
      }
    }
    /// Receives the entire data. All or nothing. Blocks until the data is received.
    pub fn recv(&self, data: &mut [u8]) {
      while !self.try_recv(data) {
        yield_();
      }
    }
    /// Trys to send the entire data. All or nothing. Returns whether the data was sent.
    pub fn try_send(&self, data: &[u8]) -> bool {
      let len = data.len();
      let head = self.head();
      let tail = self.tail();
      if len > self.capacity {
        panic!("Channel too small to send, len = {}, capacity = {}", len, self.capacity);
      }
      if tail - head + len > self.capacity {
        // println!("Channel not large enough to send, head = {}, tail = {}, len = {}, capacity = {}", head, tail, len, self.capacity);
        return false;
      }
      unsafe {
        let write_pos = tail % self.capacity;
        let buf_ptr = self.buf.add(16);
        if write_pos + len > self.capacity {
          let first_part = self.capacity - write_pos;
          core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr.add(write_pos), first_part);
          core::ptr::copy_nonoverlapping(data.as_ptr().add(first_part), buf_ptr, len - first_part);
        } else {
          core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr.add(write_pos), len);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(self.buf.add(8) as *mut usize, tail + len);
      }
      // println!("Sent {} bytes to channel", len);
      true
    }
    /// Trys to receive the entire data. All or nothing. Returns whether the data was received.
    pub fn try_recv(&self, data: &mut [u8]) -> bool {
      let len = data.len();
      let head = self.head();
      let tail = self.tail();
      if len > self.capacity {
        panic!("Channel too small to receive, len = {}, capacity = {}", len, self.capacity);
      }
      if tail - head < len {
        // println!("Channel too small to receive, head = {}, tail = {}, len = {}", head, tail, len);
        return false;
      }
      unsafe {
        let read_pos = head % self.capacity;
        let buf_ptr = self.buf.add(16);
        if read_pos + len > self.capacity {
          let first_part = self.capacity - read_pos;
          core::ptr::copy_nonoverlapping(buf_ptr.add(read_pos), data.as_mut_ptr(), first_part);
          core::ptr::copy_nonoverlapping(buf_ptr, data.as_mut_ptr().add(first_part), len - first_part);
        } else {
          core::ptr::copy_nonoverlapping(buf_ptr.add(read_pos), data.as_mut_ptr(), len);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(self.buf as *mut usize, head + len);
      }
      // println!("Received {} bytes from channel", len);
      true
    }
    /// Might only send partial data. Returns the number of bytes sent. Never blocks.
    pub fn send_bulk(&self, data: &[u8]) -> usize {
      let len = data.len();
      let head = self.head();
      let tail = self.tail();
      let available = self.capacity - (tail - head);
      let send_len = core::cmp::min(available, len);
      unsafe {
        let write_pos = tail % self.capacity;
        let buf_ptr = self.buf.add(16);
        if write_pos + send_len > self.capacity {
          let first_part = self.capacity - write_pos;
          core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr.add(write_pos), first_part);
          core::ptr::copy_nonoverlapping(data.as_ptr().add(first_part), buf_ptr, send_len - first_part);
        } else {
          core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr.add(write_pos), send_len);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(self.buf.add(8) as *mut usize, tail + send_len);
      }
      send_len
    }
    /// Might only recv partial data. Returns the number of bytes received. Never blocks.
    pub fn recv_bulk(&self, data: &mut [u8]) -> usize {
      let len = data.len();
      let head = self.head();
      let tail = self.tail();
      let available = tail - head;
      let recv_len = core::cmp::min(available, len);
      unsafe {
        let read_pos = head % self.capacity;
        let buf_ptr = self.buf.add(16);
        if read_pos + recv_len > self.capacity {
          let first_part = self.capacity - read_pos;
          core::ptr::copy_nonoverlapping(buf_ptr.add(read_pos), data.as_mut_ptr(), first_part);
          core::ptr::copy_nonoverlapping(buf_ptr, data.as_mut_ptr().add(first_part), recv_len - first_part);
        } else {
          core::ptr::copy_nonoverlapping(buf_ptr.add(read_pos), data.as_mut_ptr(), recv_len);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(self.buf as *mut usize, head + recv_len);
      }
      recv_len
    }

    pub fn is_full(&self) -> bool {
      self.tail() - self.head() == self.capacity
    }
    pub fn is_empty(&self) -> bool {
      self.head() == self.tail()
    }

    fn head(&self) -> usize {
      unsafe {
        core::ptr::read_volatile(self.buf as *mut usize)
      }
    }
    fn tail(&self) -> usize {
      unsafe {
        core::ptr::read_volatile(self.buf.add(8) as *mut usize)
      }
    }
}