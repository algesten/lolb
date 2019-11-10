use std::sync::Arc;

/// Extensions to Arc.
pub trait ArcExt<T> {
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
