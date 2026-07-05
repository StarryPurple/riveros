use super::*;

pub fn sys_ring_create(capacity: usize, vaddr_out: *mut usize) -> isize {
    let (fd, vaddr) = ring_create(capacity);
    if fd < 0 {
        return fd;
    }
    let token = crate::task::current_user_token();
    let dst = unsafe { crate::mm::translated_refmut(token, vaddr_out) };
    *dst = vaddr;
    fd
}

pub fn sys_ring_mmap(fd: usize) -> isize {
    ring_mmap(fd)
}

pub fn sys_ring_destroy(fd: usize) -> isize {
    ring_destroy(fd)
}

pub fn sys_ring_wait(fd: usize, _timeout_ms: usize) -> isize {
    ring_wait(fd)
}

pub fn sys_ring_notify(fd: usize) -> isize {
    ring_notify(fd)
}
