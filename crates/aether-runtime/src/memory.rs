use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct MemoryBudgetState {
    limit: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
}

/// Shared atomic memory budget for concurrently executing stages.
///
/// The budget accounts for planned allocations before they occur. A successful
/// reservation is represented by a non-cloneable [`MemoryReservation`] and is
/// released automatically when that guard is dropped.
#[derive(Clone, Debug)]
pub struct MemoryBudget {
    state: Arc<MemoryBudgetState>,
}

impl MemoryBudget {
    /// Creates a budget with a positive byte limit.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryBudgetError::ZeroLimit`] when `limit` is zero.
    pub fn new(limit: usize) -> Result<Self, MemoryBudgetError> {
        if limit == 0 {
            return Err(MemoryBudgetError::ZeroLimit);
        }
        Ok(Self {
            state: Arc::new(MemoryBudgetState {
                limit,
                used: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            }),
        })
    }

    /// Configured byte limit.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.state.limit
    }

    /// Bytes currently reserved by live guards.
    #[must_use]
    pub fn used(&self) -> usize {
        self.state.used.load(Ordering::Acquire)
    }

    /// Bytes currently available for a new reservation.
    #[must_use]
    pub fn available(&self) -> usize {
        self.limit().saturating_sub(self.used())
    }

    /// Highest observed concurrent reservation total.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.state.peak.load(Ordering::Acquire)
    }

    /// Atomically reserves a positive number of bytes.
    ///
    /// The compare-and-exchange loop prevents concurrent callers from jointly
    /// exceeding the limit. Callers must retain the returned guard for as long
    /// as the corresponding memory can remain allocated.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a zero request or insufficient capacity.
    pub fn try_reserve(&self, bytes: usize) -> Result<MemoryReservation, MemoryBudgetError> {
        if bytes == 0 {
            return Err(MemoryBudgetError::ZeroReservation);
        }

        let mut current = self.state.used.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(MemoryBudgetError::InsufficientCapacity {
                    requested: bytes,
                    available: self.state.limit.saturating_sub(current),
                    limit: self.state.limit,
                });
            };
            if next > self.state.limit {
                return Err(MemoryBudgetError::InsufficientCapacity {
                    requested: bytes,
                    available: self.state.limit.saturating_sub(current),
                    limit: self.state.limit,
                });
            }
            match self.state.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.state.peak.fetch_max(next, Ordering::AcqRel);
                    return Ok(MemoryReservation {
                        state: Arc::clone(&self.state),
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

/// Exclusive accounting guard for one planned allocation.
///
/// This type is intentionally not `Clone`; each successful reservation is
/// released exactly once through `Drop`.
#[derive(Debug)]
#[must_use = "the reservation guard must be held while its memory remains allocated"]
pub struct MemoryReservation {
    state: Arc<MemoryBudgetState>,
    bytes: usize,
}

impl MemoryReservation {
    /// Number of bytes held by this guard.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        self.state.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Failure to construct or reserve from a memory budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryBudgetError {
    /// A memory budget must have positive capacity.
    ZeroLimit,
    /// A reservation must represent a positive allocation.
    ZeroReservation,
    /// The requested bytes do not fit in the currently available capacity.
    InsufficientCapacity {
        /// Requested byte count.
        requested: usize,
        /// Capacity available at the failed atomic attempt.
        available: usize,
        /// Total configured limit.
        limit: usize,
    },
}

impl Display for MemoryBudgetError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroLimit => formatter.write_str("memory budget limit must be greater than zero"),
            Self::ZeroReservation => {
                formatter.write_str("memory reservation must be greater than zero")
            }
            Self::InsufficientCapacity {
                requested,
                available,
                limit,
            } => write!(
                formatter,
                "cannot reserve {requested} bytes: {available} available within {limit}-byte budget"
            ),
        }
    }
}

impl Error for MemoryBudgetError {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    #[test]
    fn validates_limit_and_reservation_size() {
        assert!(matches!(
            MemoryBudget::new(0),
            Err(MemoryBudgetError::ZeroLimit)
        ));
        let result = MemoryBudget::new(8).and_then(|budget| budget.try_reserve(0));
        assert!(matches!(result, Err(MemoryBudgetError::ZeroReservation)));
    }

    #[test]
    fn reservation_is_atomic_and_released_by_drop() -> Result<(), Box<dyn StdError>> {
        let budget = MemoryBudget::new(10)?;
        let first = budget.try_reserve(7)?;
        assert_eq!(first.bytes(), 7);
        assert_eq!(budget.used(), 7);
        assert_eq!(budget.available(), 3);
        assert!(matches!(
            budget.try_reserve(4),
            Err(MemoryBudgetError::InsufficientCapacity {
                requested: 4,
                available: 3,
                limit: 10
            })
        ));

        drop(first);
        assert_eq!(budget.used(), 0);
        let second = budget.try_reserve(10)?;
        assert_eq!(budget.peak(), 10);
        drop(second);
        assert_eq!(budget.used(), 0);
        Ok(())
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_limit() -> Result<(), Box<dyn StdError>> {
        const WORKERS: usize = 8;
        let budget = MemoryBudget::new(WORKERS)?;
        let acquired = Arc::new(Barrier::new(WORKERS + 1));
        let release = Arc::new(Barrier::new(WORKERS + 1));
        let mut workers = Vec::new();

        for _ in 0..WORKERS {
            let worker_budget = budget.clone();
            let worker_acquired = Arc::clone(&acquired);
            let worker_release = Arc::clone(&release);
            workers.push(thread::spawn(move || -> Result<(), MemoryBudgetError> {
                let reservation = worker_budget.try_reserve(1)?;
                worker_acquired.wait();
                worker_release.wait();
                drop(reservation);
                Ok(())
            }));
        }

        acquired.wait();
        assert_eq!(budget.used(), WORKERS);
        assert_eq!(budget.peak(), WORKERS);
        assert!(matches!(
            budget.try_reserve(1),
            Err(MemoryBudgetError::InsufficientCapacity { available: 0, .. })
        ));
        release.wait();
        for worker in workers {
            worker
                .join()
                .map_err(|_| std::io::Error::other("memory worker panicked"))??;
        }
        assert_eq!(budget.used(), 0);
        Ok(())
    }
}
