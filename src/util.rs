use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Extensions to Arc.
pub(crate) trait ArcExt<T> {
    /// Clones the value contained in the Arc.
    fn clone_contained(&self) -> T;
}

impl<T> ArcExt<T> for Arc<T>
where
    T: Clone,
{
    fn clone_contained(&self) -> T {
        let ptr = Arc::into_raw(Arc::clone(self));
        let v = unsafe { (*ptr).clone() };
        let _x = unsafe { Arc::from_raw(ptr) }; // bring back to avoid memory leak
        v
    }
}

pub(crate) fn current_time_millis() -> u64 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    since_the_epoch.as_millis() as u64
}
