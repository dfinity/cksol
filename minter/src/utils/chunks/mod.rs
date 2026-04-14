use itertools::Itertools;

#[cfg(test)]
mod tests;

/// A partially-applied chunking operation over an iterator.
///
/// Produced by [`IntoChunksExt::into_chunks`]; call [`take_chunks`] to
/// finish.
///
/// [`take_chunks`]: Chunked::take_chunks
pub struct Chunked<I> {
    iter: I,
    chunk_size: usize,
}

impl<'a, T, I> Chunked<I>
where
    I: Iterator<Item = &'a T>,
    T: Clone + 'a,
{
    /// Collects at most `max_chunks` chunks, discarding any remaining items.
    pub fn take_chunks(self, max_chunks: usize) -> Vec<Vec<T>> {
        let chunked = self.iter.chunks(self.chunk_size);
        chunked
            .into_iter()
            .take(max_chunks)
            .map(|chunk| chunk.cloned().collect())
            .collect()
    }
}

/// Extends iterators of references with staged chunked collection.
///
/// # Example
///
/// ```ignore
/// let data = vec![1, 2, 3, 4, 5, 6, 7];
/// let chunks = data.iter().into_chunks(3).take_chunks(2);
/// assert_eq!(chunks, vec![vec![1, 2, 3], vec![4, 5, 6]]);
/// ```
pub trait IntoChunksExt<'a, T: 'a>: Sized {
    /// Begins a chunked collection with the given chunk size.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_size` is zero.
    fn into_chunks(self, chunk_size: usize) -> Chunked<Self>;
}

impl<'a, T: 'a, I> IntoChunksExt<'a, T> for I
where
    I: Iterator<Item = &'a T>,
{
    fn into_chunks(self, chunk_size: usize) -> Chunked<Self> {
        assert!(chunk_size > 0, "chunk_size must be greater than zero");
        Chunked {
            iter: self,
            chunk_size,
        }
    }
}
