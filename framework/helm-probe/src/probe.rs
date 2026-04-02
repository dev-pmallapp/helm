use std::marker::PhantomData;

#[cfg(debug_assertions)]
type Listener<T> = Box<dyn Fn(&T) + Send + Sync>;

/// A typed probe point. Zero-sized in release; holds listeners in dev.
///
/// # Build profile behaviour
///
/// | Profile | `has_listeners()` | `notify()` | `subscribe()` |
/// |---------|------------------|------------|---------------|
/// | release (`--release`) | `const false` | empty | absent (compile error) |
/// | dev (`cargo build`)   | `!vec.is_empty()` | iterates | available |
///
/// # Design note: `PhantomData<fn(&T)>`
///
/// Using `fn(&T)` (not `T` or `*const T`) makes `Probe<T>` covariant in `T`.
/// Listeners take `&T`, so covariance is correct.
pub struct Probe<T> {
    #[cfg(debug_assertions)]
    listeners: Vec<Listener<T>>,
    _marker: PhantomData<fn(&T)>,
}

impl<T> Probe<T> {
    /// Create a probe with no listeners. Usable in const context.
    pub const fn new() -> Self {
        Self {
            #[cfg(debug_assertions)]
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// `true` iff at least one listener is subscribed.
    ///
    /// Release: const `false` -- compiler eliminates `if probe.has_listeners()` blocks.
    /// Dev: `!vec.is_empty()` -- one load + compare, predicted-not-taken.
    pub fn has_listeners(&self) -> bool {
        #[cfg(not(debug_assertions))]
        {
            false
        }
        #[cfg(debug_assertions)]
        {
            !self.listeners.is_empty()
        }
    }

    /// Deliver event to all listeners. No-op in release (empty body, inlined away).
    pub fn notify(&self, val: &T) {
        #[cfg(debug_assertions)]
        for l in &self.listeners {
            l(val);
        }
        let _ = val;
    }

    /// Subscribe a listener closure.
    ///
    /// Only available in debug builds. In release, calling this method is a
    /// **compile error** (`method not found`), preventing subscriptions that
    /// would be silently discarded.
    #[cfg(debug_assertions)]
    pub fn subscribe(&mut self, f: impl Fn(&T) + Send + Sync + 'static) {
        self.listeners.push(Box::new(f));
    }

    /// Number of registered listeners. Only available in debug builds.
    #[cfg(debug_assertions)]
    pub fn listener_count(&self) -> usize {
        self.listeners.len()
    }
}

impl<T> Default for Probe<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Vec<Box<dyn Fn(&T) + Send + Sync>> is Send+Sync because the closure
// bound requires Send+Sync. PhantomData<fn(&T)> is not auto Send+Sync (raw fn ptr),
// so we implement manually. In release there are no non-PhantomData fields.
unsafe impl<T> Send for Probe<T> {}
unsafe impl<T> Sync for Probe<T> {}
