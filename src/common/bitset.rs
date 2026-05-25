//! Compact, fixed-capacity bit set keyed by `usize` indices.
//!
//! Used by dataflow analyses and the register allocator's interference graph
//! whenever the index domain is dense and small enough to fit in a
//! `Vec<u64>` (one bit per index).  Set operations are word-parallel and
//! roughly 64× faster than the equivalent `HashSet<usize>` operations on
//! dense data.

use std::fmt;

/// A bit set storing membership for indices in `0..capacity`.
///
/// The capacity is fixed at construction time.  All operations that combine
/// two bit sets require both operands to have the same capacity.
#[derive(Clone, Eq)]
pub struct Bitset {
    words: Vec<u64>,
    capacity: usize,
}

impl Bitset {
    /// Creates an empty bit set capable of storing indices in `0..capacity`.
    pub fn new(capacity: usize) -> Self {
        let num_words = capacity.div_ceil(64);
        Self {
            words: vec![0; num_words],
            capacity,
        }
    }

    /// Inserts `i` into the set.  Returns `true` if the bit was not
    /// previously set.
    ///
    /// Panics if `i >= capacity`.
    pub fn insert(&mut self, i: usize) -> bool {
        let (w, mask) = self.locate(i);
        let was_set = self.words[w] & mask != 0;
        self.words[w] |= mask;
        !was_set
    }

    /// Removes `i` from the set.  Returns `true` if the bit was previously
    /// set.
    ///
    /// Panics if `i >= capacity`.
    pub fn remove(&mut self, i: usize) -> bool {
        let (w, mask) = self.locate(i);
        let was_set = self.words[w] & mask != 0;
        self.words[w] &= !mask;
        was_set
    }

    /// Returns whether `i` is in the set.  Indices `>= capacity` are
    /// reported as absent.
    pub fn contains(&self, i: usize) -> bool {
        if i >= self.capacity {
            return false;
        }
        let (w, mask) = self.locate(i);
        self.words[w] & mask != 0
    }

    /// Removes all elements.
    pub fn clear(&mut self) {
        for w in &mut self.words {
            *w = 0;
        }
    }

    /// Returns the number of elements (population count).
    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Returns whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    /// Computes `self |= other` in place.  Returns `true` if any bit was
    /// newly set.
    pub fn union_with(&mut self, other: &Self) -> bool {
        debug_assert_eq!(self.capacity, other.capacity);
        let mut changed = false;
        for (a, &b) in self.words.iter_mut().zip(other.words.iter()) {
            let new = *a | b;
            if new != *a {
                changed = true;
                *a = new;
            }
        }
        changed
    }

    /// Sets `self` to `gen ∪ (out ∖ kill)`, the canonical backward-liveness
    /// transfer function.  Returns `true` if `self` changed.
    pub fn assign_transfer(&mut self, gen: &Self, kill: &Self, out: &Self) -> bool {
        debug_assert_eq!(self.capacity, gen.capacity);
        debug_assert_eq!(self.capacity, kill.capacity);
        debug_assert_eq!(self.capacity, out.capacity);
        let mut changed = false;
        for i in 0..self.words.len() {
            let new = gen.words[i] | (out.words[i] & !kill.words[i]);
            if new != self.words[i] {
                changed = true;
                self.words[i] = new;
            }
        }
        changed
    }

    /// Iterates over the indices of set bits in ascending order.
    pub fn iter(&self) -> Iter<'_> {
        let current = self.words.first().copied().unwrap_or(0);
        Iter {
            words: &self.words,
            word_idx: 0,
            current,
        }
    }

    fn locate(&self, i: usize) -> (usize, u64) {
        assert!(
            i < self.capacity,
            "bit index {i} out of bounds (capacity {})",
            self.capacity
        );
        (i / 64, 1u64 << (i % 64))
    }
}

impl PartialEq for Bitset {
    fn eq(&self, other: &Self) -> bool {
        self.capacity == other.capacity && self.words == other.words
    }
}

impl fmt::Debug for Bitset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Ascending iterator over the set bits of a [`Bitset`].
pub struct Iter<'a> {
    words: &'a [u64],
    word_idx: usize,
    current: u64,
}

impl<'a> Iterator for Iter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        loop {
            if self.current != 0 {
                let b = self.current.trailing_zeros() as usize;
                self.current &= self.current - 1;
                return Some(self.word_idx * 64 + b);
            }
            self.word_idx += 1;
            if self.word_idx >= self.words.len() {
                return None;
            }
            self.current = self.words[self.word_idx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_contains_clear() {
        let mut b = Bitset::new(200);
        assert!(b.is_empty());
        assert!(!b.contains(5));
        assert!(b.insert(5));
        assert!(!b.insert(5));
        assert!(b.contains(5));
        assert!(b.insert(199));
        assert_eq!(b.len(), 2);

        b.clear();
        assert!(b.is_empty());
        assert!(!b.contains(5));
    }

    #[test]
    fn iteration_is_sorted() {
        let mut b = Bitset::new(300);
        for i in [0, 63, 64, 127, 128, 256, 299] {
            b.insert(i);
        }
        let collected: Vec<usize> = b.iter().collect();
        assert_eq!(collected, vec![0, 63, 64, 127, 128, 256, 299]);
    }

    #[test]
    fn union_with_change_tracking() {
        let mut a = Bitset::new(128);
        let mut other = Bitset::new(128);
        a.insert(1);
        a.insert(2);
        other.insert(2);
        other.insert(3);

        assert!(a.union_with(&other));
        let collected: Vec<usize> = a.iter().collect();
        assert_eq!(collected, vec![1, 2, 3]);

        assert!(!a.union_with(&other));
    }

    #[test]
    fn assign_transfer_matches_definition() {
        let cap = 130;
        let mut gen = Bitset::new(cap);
        let mut kill = Bitset::new(cap);
        let mut out = Bitset::new(cap);
        let mut result = Bitset::new(cap);

        gen.insert(1);
        gen.insert(100);
        kill.insert(2);
        kill.insert(64);
        out.insert(2);
        out.insert(3);
        out.insert(64);
        out.insert(129);

        assert!(result.assign_transfer(&gen, &kill, &out));
        let collected: Vec<usize> = result.iter().collect();
        assert_eq!(collected, vec![1, 3, 100, 129]);

        assert!(!result.assign_transfer(&gen, &kill, &out));
    }

    #[test]
    fn out_of_range_contains_is_false() {
        let b = Bitset::new(10);
        assert!(!b.contains(10));
        assert!(!b.contains(usize::MAX));
    }

    #[test]
    fn remove_clears_individual_bits() {
        let mut b = Bitset::new(200);
        b.insert(0);
        b.insert(63);
        b.insert(64);
        b.insert(128);

        assert!(b.remove(64));
        assert!(!b.remove(64));
        assert!(b.contains(0));
        assert!(b.contains(63));
        assert!(!b.contains(64));
        assert!(b.contains(128));
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn equality_compares_capacity_and_bits() {
        let mut a = Bitset::new(64);
        let mut b = Bitset::new(64);
        a.insert(0);
        b.insert(0);
        assert_eq!(a, b);

        let c = Bitset::new(128);
        assert_ne!(a, c);
    }
}
