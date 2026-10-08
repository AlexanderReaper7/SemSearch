use std::collections::HashMap;

// First.
pub fn one(x: u32) -> u32 {
    let y = x + 1;
    y * 2
}

/// Second.
#[inline]
pub fn two(items: &[u32]) -> HashMap<u32, usize> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(*item).or_default() += 1;
    }
    counts
}
