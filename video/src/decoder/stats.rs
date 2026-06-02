use portable_atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub struct SumStats(AtomicUsize);

impl SumStats {
    pub fn push(&self, val: usize) {
        let _ = self.0.add(val, Ordering::Relaxed);
    }

    pub fn take_sum(&self) -> usize {
        self.0.swap(0, Ordering::Relaxed)
    }
}
