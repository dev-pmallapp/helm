use std::cell::UnsafeCell;

pub struct Scoreboard<T> {
    slots: Vec<UnsafeCell<T>>,
}

unsafe impl<T: Send> Sync for Scoreboard<T> {}
unsafe impl<T: Send> Send for Scoreboard<T> {}

impl<T: Default> Scoreboard<T> {
    pub fn new(n: usize) -> Self {
        Self { slots: (0..n).map(|_| UnsafeCell::new(T::default())).collect() }
    }
    pub fn get(&self, idx: usize) -> &T { unsafe { &*self.slots[idx].get() } }
    #[allow(clippy::mut_from_ref)]
    pub fn get_mut(&self, idx: usize) -> &mut T { unsafe { &mut *self.slots[idx].get() } }
    pub fn len(&self) -> usize { self.slots.len() }
    pub fn is_empty(&self) -> bool { self.slots.is_empty() }
    pub fn iter(&self) -> impl Iterator<Item = &T> { self.slots.iter().map(|c| unsafe { &*c.get() }) }
}
