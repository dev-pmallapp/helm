//! Guest file-descriptor table mapping guest fds to host fds.

use std::collections::HashMap;

pub struct FdTable {
    map: HashMap<i32, i32>, // guest_fd -> host_fd
    next_fd: i32,
}

impl FdTable {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        // stdout/stderr pass through to host; stdin reads /dev/null
        // so guest read(0, ...) returns EOF instead of blocking.
        let null_fd = unsafe { libc::open(b"/dev/null\0".as_ptr().cast(), libc::O_RDONLY) };
        map.insert(0, if null_fd >= 0 { null_fd } else { 0 });
        map.insert(1, 1);
        map.insert(2, 2);
        Self { map, next_fd: 3 }
    }

    pub fn get_host_fd(&self, guest_fd: i32) -> Option<i32> {
        self.map.get(&guest_fd).copied()
    }

    pub fn alloc(&mut self, host_fd: i32) -> i32 {
        let guest_fd = self.next_fd;
        self.next_fd += 1;
        self.map.insert(guest_fd, host_fd);
        guest_fd
    }

    pub fn close(&mut self, guest_fd: i32) -> bool {
        self.map.remove(&guest_fd).is_some()
    }

    pub fn dup(&mut self, old_guest_fd: i32) -> Option<i32> {
        let host_fd = *self.map.get(&old_guest_fd)?;
        Some(self.alloc(host_fd))
    }

    pub fn dup_to(&mut self, old_guest_fd: i32, new_guest_fd: i32) -> Option<i32> {
        let host_fd = *self.map.get(&old_guest_fd)?;
        self.map.insert(new_guest_fd, host_fd);
        if new_guest_fd >= self.next_fd {
            self.next_fd = new_guest_fd + 1;
        }
        Some(new_guest_fd)
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}
