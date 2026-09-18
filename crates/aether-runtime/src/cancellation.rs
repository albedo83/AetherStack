use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared, monotonic cancellation signal for cooperative pipeline stages.
///
/// Clones observe the same state. Cancellation is permanent: a token cannot be
/// reset and therefore cannot accidentally be reused for unrelated work.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates an active cancellation token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation and returns `true` only for the first request.
    ///
    /// `AcqRel` publishes writes performed before cancellation and pairs with
    /// the acquiring checks made by worker threads.
    #[must_use]
    pub fn cancel(&self) -> bool {
        !self.cancelled.swap(true, Ordering::AcqRel)
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Cooperative checkpoint used between bounded units of work.
    ///
    /// # Errors
    ///
    /// Returns [`Cancelled`] once any clone has requested cancellation.
    pub fn checkpoint(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Error returned by a cooperative cancellation checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

impl Display for Cancelled {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("pipeline execution was cancelled")
    }
}

impl Error for Cancelled {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::thread;

    use super::*;

    #[test]
    fn cancellation_is_shared_monotonic_and_idempotent() -> Result<(), Box<dyn StdError>> {
        let token = CancellationToken::new();
        let worker = token.clone();
        assert_eq!(worker.checkpoint(), Ok(()));

        let observed = thread::spawn(move || {
            let first = worker.cancel();
            (first, worker.is_cancelled(), worker.checkpoint())
        })
        .join()
        .map_err(|_| std::io::Error::other("cancellation worker panicked"))?;

        assert!(observed.0);
        assert!(observed.1);
        assert_eq!(observed.2, Err(Cancelled));
        assert!(token.is_cancelled());
        assert!(!token.cancel());
        assert_eq!(token.checkpoint(), Err(Cancelled));
        Ok(())
    }

    #[test]
    fn independent_tokens_do_not_share_state() {
        let first = CancellationToken::new();
        let second = CancellationToken::new();

        assert!(first.cancel());
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }
}
