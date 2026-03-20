use std::{
    iter,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Stubs<T>(Arc<Mutex<Box<dyn Iterator<Item = T> + Send>>>);

impl<T: 'static + Send> Stubs<T> {
    pub fn next(&self) -> T {
        self.0
            .try_lock()
            .unwrap()
            .next()
            .expect("No more stub values!")
    }

    pub fn chain<I>(self, other: I) -> Self
    where
        I: IntoIterator<Item = T> + 'static,
        I::IntoIter: Send,
    {
        let old_iter = Arc::into_inner(self.0).unwrap().into_inner().unwrap();
        Self(Arc::new(Mutex::new(Box::new(old_iter.chain(other)))))
    }

    pub fn add(self, value: T) -> Self {
        self.chain(iter::once(value))
    }
}

impl<T: 'static + Send> Default for Stubs<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Box::new(iter::empty()))))
    }
}

impl<T, I> From<I> for Stubs<T>
where
    T: 'static,
    I: IntoIterator<Item = T, IntoIter: Send> + 'static,
{
    fn from(stubs: I) -> Self {
        Self(Arc::new(Mutex::new(Box::new(stubs.into_iter()))))
    }
}
