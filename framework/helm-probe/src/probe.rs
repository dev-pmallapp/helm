use std::marker::PhantomData;

#[cfg(feature = "instrumentation")]
type Listener<T> = Box<dyn Fn(&T) + Send + Sync>;

/// A typed probe point. Zero-sized by default; holds listeners when the
/// `instrumentation` feature is enabled.
///
/// # Build profile behaviour
///
/// | Build | `has_listeners()` | `notify()` | `subscribe()` |
/// |------|------------------|------------|---------------|
/// | default | `const false` | empty | absent (compile error) |
/// | `--features instrumentation` | `!vec.is_empty()` | iterates | available |
///
/// # Design note: `PhantomData<fn(&T)>`
///
/// Using `fn(&T)` (not `T` or `*const T`) makes `Probe<T>` covariant in `T`.
/// Listeners take `&T`, so covariance is correct.
pub struct Probe<T> {
    #[cfg(feature = "instrumentation")]
    listeners: Vec<Listener<T>>,
    _marker: PhantomData<fn(&T)>,
}

impl<T> Probe<T> {
    /// Create a probe with no listeners. Usable in const context.
    pub const fn new() -> Self {
        Self {
            #[cfg(feature = "instrumentation")]
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// `true` iff at least one listener is subscribed.
    ///
    /// Default: const `false` -- compiler eliminates `if probe.has_listeners()` blocks.
    /// With `instrumentation`: `!vec.is_empty()` -- one load + compare, predicted-not-taken.
    pub fn has_listeners(&self) -> bool {
        #[cfg(not(feature = "instrumentation"))]
        {
            false
        }
        #[cfg(feature = "instrumentation")]
        {
            !self.listeners.is_empty()
        }
    }

    /// Deliver event to all listeners. No-op unless `instrumentation` is enabled.
    pub fn notify(&self, val: &T) {
        #[cfg(feature = "instrumentation")]
        for l in &self.listeners {
            l(val);
        }
        let _ = val;
    }

    /// Subscribe a listener closure.
    ///
    /// Only available with the `instrumentation` feature. Without it, calling this method is a
    /// **compile error** (`method not found`), preventing subscriptions that
    /// would be silently discarded.
    #[cfg(feature = "instrumentation")]
    pub fn subscribe(&mut self, f: impl Fn(&T) + Send + Sync + 'static) {
        self.listeners.push(Box::new(f));
    }

    /// Number of registered listeners. Only available with the `instrumentation` feature.
    #[cfg(feature = "instrumentation")]
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
// so we implement manually. Without instrumentation there are no non-PhantomData fields.
unsafe impl<T> Send for Probe<T> {}
unsafe impl<T> Sync for Probe<T> {}
