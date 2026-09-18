use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

/// Maximum UTF-8 byte length of a canonical stage identifier.
pub const MAX_STAGE_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a progress failure or cancellation code.
pub const MAX_PROGRESS_CODE_BYTES: usize = 128;

/// Canonical machine-readable identifier for one pipeline stage.
///
/// Identifiers start with a lowercase ASCII letter or digit. Remaining bytes may
/// additionally contain `.`, `_`, and `-`. This restricted form is portable
/// across JSON, logs, cache keys, and command-line progress adapters.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct StageId(String);

impl StageId {
    /// Validates and stores a canonical stage identifier.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty, oversized, or non-canonical value.
    pub fn new(value: impl Into<String>) -> Result<Self, StageIdError> {
        let value = value.into();
        validate_stage_id(&value)?;
        Ok(Self(value))
    }

    /// Canonical identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for StageId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Failure to construct a canonical [`StageId`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageIdError {
    /// The identifier is empty.
    Empty,
    /// The identifier exceeds the stable byte limit.
    TooLong {
        /// Observed UTF-8 byte length.
        length: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// The first byte is not a lowercase ASCII letter or digit.
    InvalidStart {
        /// Rejected byte.
        byte: u8,
    },
    /// A later byte is outside the canonical character set.
    InvalidByte {
        /// Zero-based byte offset.
        index: usize,
        /// Rejected byte.
        byte: u8,
    },
}

impl Display for StageIdError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("stage identifier must not be empty"),
            Self::TooLong { length, maximum } => write!(
                formatter,
                "stage identifier has {length} bytes; maximum is {maximum}"
            ),
            Self::InvalidStart { byte } => write!(
                formatter,
                "stage identifier starts with invalid byte 0x{byte:02x}"
            ),
            Self::InvalidByte { index, byte } => write!(
                formatter,
                "stage identifier contains invalid byte 0x{byte:02x} at offset {index}"
            ),
        }
    }
}

impl Error for StageIdError {}

/// Lifecycle state carried by a progress event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    /// Stage accepted by the runtime but no unit completed yet.
    Started,
    /// Stage has completed zero or more bounded units.
    Running,
    /// Stage finished successfully.
    Completed,
    /// Stage stopped after cooperative cancellation.
    Cancelled,
    /// Stage terminated with a typed external error code.
    Failed,
}

/// Validated, machine-readable snapshot of one stage's progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressEvent {
    sequence: u64,
    stage: StageId,
    state: ProgressState,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressEventWire {
    sequence: u64,
    stage: StageId,
    state: ProgressState,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<String>,
}

impl ProgressEvent {
    /// Builds a validated progress event.
    ///
    /// Failure and cancellation states require a canonical code. Other states
    /// reject one. Known totals are positive, completed units cannot exceed the
    /// total, started stages are at zero, and completed stages reach their total.
    ///
    /// # Errors
    ///
    /// Returns a typed invariant violation for inconsistent event fields.
    pub fn new(
        sequence: u64,
        stage: StageId,
        state: ProgressState,
        completed_units: u64,
        total_units: Option<u64>,
        code: Option<String>,
    ) -> Result<Self, ProgressEventError> {
        validate_event(
            sequence,
            state,
            completed_units,
            total_units,
            code.as_deref(),
        )?;
        Ok(Self {
            sequence,
            stage,
            state,
            completed_units,
            total_units,
            code,
        })
    }

    /// Monotonic event sequence assigned by one runtime stream.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Stage associated with this event.
    #[must_use]
    pub const fn stage(&self) -> &StageId {
        &self.stage
    }

    /// Lifecycle state of the stage.
    #[must_use]
    pub const fn state(&self) -> ProgressState {
        self.state
    }

    /// Completed deterministic work units.
    #[must_use]
    pub const fn completed_units(&self) -> u64 {
        self.completed_units
    }

    /// Total work units, when knowable before completion.
    #[must_use]
    pub const fn total_units(&self) -> Option<u64> {
        self.total_units
    }

    /// Stable failure or cancellation code.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }
}

impl<'de> Deserialize<'de> for ProgressEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ProgressEventWire::deserialize(deserializer)?;
        Self::new(
            wire.sequence,
            wire.stage,
            wire.state,
            wire.completed_units,
            wire.total_units,
            wire.code,
        )
        .map_err(D::Error::custom)
    }
}

/// Atomic sequence allocator for one progress stream.
#[derive(Debug, Default)]
pub struct ProgressSequence {
    current: AtomicU64,
}

impl ProgressSequence {
    /// Creates a sequence whose first valid event receives number one.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
        }
    }

    /// Last sequence number assigned, or zero before the first event.
    #[must_use]
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::Acquire)
    }

    /// Validates event fields and atomically assigns the next sequence number.
    ///
    /// Invalid fields do not consume a sequence number. Concurrent successful
    /// calls receive distinct values, although downstream delivery order remains
    /// the responsibility of the progress adapter.
    ///
    /// # Errors
    ///
    /// Returns an event invariant error or sequence exhaustion.
    pub fn next(
        &self,
        stage: StageId,
        state: ProgressState,
        completed_units: u64,
        total_units: Option<u64>,
        code: Option<String>,
    ) -> Result<ProgressEvent, ProgressEventError> {
        validate_event(1, state, completed_units, total_units, code.as_deref())?;
        let previous = self
            .current
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| ProgressEventError::SequenceExhausted)?;
        let sequence = previous
            .checked_add(1)
            .ok_or(ProgressEventError::SequenceExhausted)?;
        ProgressEvent::new(sequence, stage, state, completed_units, total_units, code)
    }
}

/// Failure to construct a coherent progress event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressEventError {
    /// Serialized events must use a positive sequence number.
    ZeroSequence,
    /// A known total must contain at least one work unit.
    ZeroTotal,
    /// Completed units exceed the declared total.
    CompletedExceedsTotal {
        /// Completed work units.
        completed: u64,
        /// Declared total work units.
        total: u64,
    },
    /// A started event must report zero completed units.
    StartedAfterProgress,
    /// A completed event with a known total must reach that total exactly.
    IncompleteCompletion,
    /// A non-failure state carried a failure code.
    UnexpectedCode,
    /// A failed or cancelled event omitted its stable code.
    MissingCode,
    /// A code was empty, oversized, or contained a non-canonical byte.
    InvalidCode,
    /// No further sequence number can be represented by `u64`.
    SequenceExhausted,
}

impl Display for ProgressEventError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroSequence => {
                formatter.write_str("progress sequence must be greater than zero")
            }
            Self::ZeroTotal => formatter.write_str("progress total must be greater than zero"),
            Self::CompletedExceedsTotal { completed, total } => write!(
                formatter,
                "completed progress {completed} exceeds declared total {total}"
            ),
            Self::StartedAfterProgress => {
                formatter.write_str("started progress must have zero completed units")
            }
            Self::IncompleteCompletion => {
                formatter.write_str("completed progress must equal its declared total")
            }
            Self::UnexpectedCode => {
                formatter.write_str("only failed or cancelled progress may carry a code")
            }
            Self::MissingCode => {
                formatter.write_str("failed or cancelled progress requires a code")
            }
            Self::InvalidCode => formatter.write_str("progress code is not canonical"),
            Self::SequenceExhausted => formatter.write_str("progress sequence is exhausted"),
        }
    }
}

impl Error for ProgressEventError {}

fn validate_stage_id(value: &str) -> Result<(), StageIdError> {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first().copied() else {
        return Err(StageIdError::Empty);
    };
    if bytes.len() > MAX_STAGE_ID_BYTES {
        return Err(StageIdError::TooLong {
            length: bytes.len(),
            maximum: MAX_STAGE_ID_BYTES,
        });
    }
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(StageIdError::InvalidStart { byte: first });
    }
    for (index, byte) in bytes.iter().copied().enumerate().skip(1) {
        if !byte.is_ascii_lowercase()
            && !byte.is_ascii_digit()
            && !matches!(byte, b'.' | b'_' | b'-')
        {
            return Err(StageIdError::InvalidByte { index, byte });
        }
    }
    Ok(())
}

fn validate_event(
    sequence: u64,
    state: ProgressState,
    completed_units: u64,
    total_units: Option<u64>,
    code: Option<&str>,
) -> Result<(), ProgressEventError> {
    if sequence == 0 {
        return Err(ProgressEventError::ZeroSequence);
    }
    if total_units == Some(0) {
        return Err(ProgressEventError::ZeroTotal);
    }
    if let Some(total) = total_units
        && completed_units > total
    {
        return Err(ProgressEventError::CompletedExceedsTotal {
            completed: completed_units,
            total,
        });
    }
    if state == ProgressState::Started && completed_units != 0 {
        return Err(ProgressEventError::StartedAfterProgress);
    }
    if state == ProgressState::Completed
        && total_units.is_some_and(|total| completed_units != total)
    {
        return Err(ProgressEventError::IncompleteCompletion);
    }

    let requires_code = matches!(state, ProgressState::Cancelled | ProgressState::Failed);
    match (requires_code, code) {
        (true, None) => return Err(ProgressEventError::MissingCode),
        (false, Some(_)) => return Err(ProgressEventError::UnexpectedCode),
        (true, Some(value)) if !valid_code(value) => {
            return Err(ProgressEventError::InvalidCode);
        }
        _ => {}
    }
    Ok(())
}

fn valid_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_PROGRESS_CODE_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().copied().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn stage() -> Result<StageId, StageIdError> {
        StageId::new("integration.tile")
    }

    #[test]
    fn stage_ids_enforce_one_portable_canonical_form() {
        assert!(StageId::new("ingest.fits_01-a").is_ok());
        assert!(matches!(StageId::new(""), Err(StageIdError::Empty)));
        assert!(matches!(
            StageId::new("Integration"),
            Err(StageIdError::InvalidStart { .. })
        ));
        assert!(matches!(
            StageId::new("integration/tile"),
            Err(StageIdError::InvalidByte { .. })
        ));
        assert!(matches!(
            StageId::new("a".repeat(MAX_STAGE_ID_BYTES + 1)),
            Err(StageIdError::TooLong { .. })
        ));
    }

    #[test]
    fn event_invariants_cover_each_lifecycle_rule() -> Result<(), Box<dyn StdError>> {
        assert!(matches!(
            ProgressEvent::new(0, stage()?, ProgressState::Started, 0, Some(4), None),
            Err(ProgressEventError::ZeroSequence)
        ));
        assert!(matches!(
            ProgressEvent::new(1, stage()?, ProgressState::Started, 0, Some(0), None),
            Err(ProgressEventError::ZeroTotal)
        ));
        assert!(matches!(
            ProgressEvent::new(1, stage()?, ProgressState::Running, 5, Some(4), None),
            Err(ProgressEventError::CompletedExceedsTotal { .. })
        ));
        assert!(matches!(
            ProgressEvent::new(1, stage()?, ProgressState::Started, 1, Some(4), None),
            Err(ProgressEventError::StartedAfterProgress)
        ));
        assert!(matches!(
            ProgressEvent::new(1, stage()?, ProgressState::Completed, 3, Some(4), None),
            Err(ProgressEventError::IncompleteCompletion)
        ));
        assert!(matches!(
            ProgressEvent::new(1, stage()?, ProgressState::Failed, 3, Some(4), None),
            Err(ProgressEventError::MissingCode)
        ));
        assert!(matches!(
            ProgressEvent::new(
                1,
                stage()?,
                ProgressState::Running,
                3,
                Some(4),
                Some("unexpected".to_owned())
            ),
            Err(ProgressEventError::UnexpectedCode)
        ));
        assert!(matches!(
            ProgressEvent::new(
                1,
                stage()?,
                ProgressState::Failed,
                3,
                Some(4),
                Some("Not Portable".to_owned())
            ),
            Err(ProgressEventError::InvalidCode)
        ));
        Ok(())
    }

    #[test]
    fn json_round_trip_revalidates_private_state() -> Result<(), Box<dyn StdError>> {
        let event = ProgressEvent::new(
            7,
            stage()?,
            ProgressState::Failed,
            3,
            Some(4),
            Some("source.invalid".to_owned()),
        )?;
        let encoded = serde_json::to_vec(&event)?;
        let decoded: ProgressEvent = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, event);

        let tampered = br#"{"sequence":7,"stage":"integration.tile","state":"completed","completed_units":3,"total_units":4,"code":null}"#;
        assert!(serde_json::from_slice::<ProgressEvent>(tampered).is_err());
        let unknown = br#"{"sequence":7,"stage":"integration.tile","state":"running","completed_units":3,"total_units":4,"code":null,"extra":true}"#;
        assert!(serde_json::from_slice::<ProgressEvent>(unknown).is_err());
        let invalid_stage = br#"{"sequence":7,"stage":"Integration","state":"running","completed_units":3,"total_units":4,"code":null}"#;
        assert!(serde_json::from_slice::<ProgressEvent>(invalid_stage).is_err());
        Ok(())
    }

    #[test]
    fn sequence_is_atomic_and_invalid_events_do_not_consume_numbers()
    -> Result<(), Box<dyn StdError>> {
        const WORKERS: usize = 16;
        let sequence = Arc::new(ProgressSequence::new());
        assert!(matches!(
            sequence.next(stage()?, ProgressState::Started, 1, Some(2), None),
            Err(ProgressEventError::StartedAfterProgress)
        ));
        assert_eq!(sequence.current(), 0);

        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let worker_sequence = Arc::clone(&sequence);
            let worker_stage = stage()?;
            workers.push(thread::spawn(move || {
                worker_sequence.next(worker_stage, ProgressState::Running, 1, Some(2), None)
            }));
        }
        let mut assigned = Vec::new();
        for worker in workers {
            let event = worker
                .join()
                .map_err(|_| std::io::Error::other("progress worker panicked"))??;
            assigned.push(event.sequence());
        }
        assigned.sort_unstable();
        assert_eq!(assigned, (1..=WORKERS as u64).collect::<Vec<_>>());
        assert_eq!(sequence.current(), WORKERS as u64);
        Ok(())
    }

    #[test]
    fn accepts_each_coherent_terminal_state() -> Result<(), Box<dyn StdError>> {
        let completed =
            ProgressEvent::new(1, stage()?, ProgressState::Completed, 4, Some(4), None)?;
        let cancelled = ProgressEvent::new(
            2,
            stage()?,
            ProgressState::Cancelled,
            2,
            Some(4),
            Some("cancelled".to_owned()),
        )?;
        assert_eq!(completed.state(), ProgressState::Completed);
        assert_eq!(cancelled.code(), Some("cancelled"));
        Ok(())
    }

    #[test]
    fn reports_sequence_exhaustion_without_wrapping() -> Result<(), Box<dyn StdError>> {
        let sequence = ProgressSequence::new();
        sequence.current.store(u64::MAX, Ordering::Release);

        assert!(matches!(
            sequence.next(stage()?, ProgressState::Running, 1, None, None),
            Err(ProgressEventError::SequenceExhausted)
        ));
        assert_eq!(sequence.current(), u64::MAX);
        Ok(())
    }
}
