/// Fire a probe, constructing the event value only when listeners exist.
///
/// Without the `instrumentation` feature: expands to nothing.
/// In dev builds: expands to:
/// ```rust,ignore
/// if $probe.has_listeners() {
///     $probe.notify(&{ $val });
/// }
/// ```
///
/// The `{ $val }` block is only evaluated when listeners exist.
#[macro_export]
macro_rules! probe {
    ($probe:expr, $val:expr) => {
        if $probe.has_listeners() {
            $probe.notify(&{ $val });
        }
    };
}
