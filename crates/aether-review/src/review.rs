use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Maximum number of frames accepted by one interactive review model.
pub const MAX_REVIEW_FRAMES: usize = 1_000_000;
/// Maximum number of decisions in one previewed transaction.
pub const MAX_BATCH_CHANGES: usize = 100_000;
/// Maximum retained undo transactions.
pub const MAX_UNDO_DEPTH: usize = 1_024;
/// Maximum UTF-8 byte length of a user-visible frame label.
pub const MAX_LABEL_BYTES: usize = 4_096;
/// Maximum UTF-8 byte length of a manual review note.
pub const MAX_NOTE_BYTES: usize = 4_096;
/// Maximum UTF-8 byte length accepted for a portable source path.
pub const MAX_SOURCE_PATH_BYTES: usize = 4_096;

const FRAME_ID_DOMAIN: &[u8] = b"aether-review-frame-id-v1\0";

/// Stable content-derived identity used by the review interface.
///
/// A raw content fingerprint is insufficient because two separate manifest
/// entries may contain identical bytes. [`FrameId::derive`] domain-separates the
/// portable session-relative path, byte length, and content fingerprint while
/// remaining independent of the machine-specific session root.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FrameId(String);

impl FrameId {
    /// Validates a canonical lowercase SHA-256 frame identity.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::InvalidFrameId`] unless `value` is exactly 64
    /// lowercase hexadecimal ASCII characters.
    pub fn new(value: impl Into<String>) -> Result<Self, ReviewError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReviewError::InvalidFrameId);
        }
        Ok(Self(value))
    }

    /// Derives a unique portable identity from one validated session source.
    ///
    /// The exact versioned encoding is: domain bytes, big-endian path byte
    /// length, UTF-8 path bytes, big-endian source byte length, and the canonical
    /// lowercase content-digest ASCII bytes. Moving the whole session does not
    /// change the identity; adding a byte-identical source at another relative
    /// path does.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a non-portable or oversized relative path, zero
    /// source length, or a non-canonical SHA-256 content fingerprint.
    pub fn derive(
        portable_relative_path: &str,
        byte_length: u64,
        content_sha256: &str,
    ) -> Result<Self, ReviewError> {
        if !valid_portable_relative_path(portable_relative_path) {
            return Err(ReviewError::InvalidSourcePath);
        }
        if byte_length == 0 || !valid_sha256(content_sha256) {
            return Err(ReviewError::InvalidSourceFingerprint);
        }
        let path_length = u64::try_from(portable_relative_path.len())
            .map_err(|_| ReviewError::InvalidSourcePath)?;
        let mut hasher = Sha256::new();
        hasher.update(FRAME_ID_DOMAIN);
        hasher.update(path_length.to_be_bytes());
        hasher.update(portable_relative_path.as_bytes());
        hasher.update(byte_length.to_be_bytes());
        hasher.update(content_sha256.as_bytes());
        Ok(Self(encode_lower_hex(&hasher.finalize())))
    }

    /// Canonical lowercase hexadecimal identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Optional quality columns exposed by the review table.
///
/// Missing values remain `None`; they are never converted to numerical zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct FrameMetrics {
    background: Option<f64>,
    noise: Option<f64>,
    detected_stars: Option<usize>,
    usable_stars: Option<usize>,
    fwhm_major_pixels: Option<f64>,
    eccentricity: Option<f64>,
}

impl FrameMetrics {
    /// Creates a validated review-metric summary.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::InvalidMetric`] for a non-finite value, negative
    /// noise or FWHM, eccentricity outside `[0, 1]`, or usable-star count larger
    /// than the detected-star count.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        background: Option<f64>,
        noise: Option<f64>,
        detected_stars: Option<usize>,
        usable_stars: Option<usize>,
        fwhm_major_pixels: Option<f64>,
        eccentricity: Option<f64>,
    ) -> Result<Self, ReviewError> {
        validate_optional_finite("background", background, |_| true)?;
        validate_optional_finite("noise", noise, |value| value >= 0.0)?;
        validate_optional_finite("fwhm_major_pixels", fwhm_major_pixels, |value| value >= 0.0)?;
        validate_optional_finite("eccentricity", eccentricity, |value| {
            (0.0..=1.0).contains(&value)
        })?;
        if matches!((detected_stars, usable_stars), (Some(total), Some(usable)) if usable > total) {
            return Err(ReviewError::InvalidMetric {
                field: "usable_stars",
            });
        }
        Ok(Self {
            background,
            noise,
            detected_stars,
            usable_stars,
            fwhm_major_pixels,
            eccentricity,
        })
    }

    /// Robust background location.
    #[must_use]
    pub const fn background(self) -> Option<f64> {
        self.background
    }

    /// Robust background-noise estimate.
    #[must_use]
    pub const fn noise(self) -> Option<f64> {
        self.noise
    }

    /// Local maxima found before measurement filtering.
    #[must_use]
    pub const fn detected_stars(self) -> Option<usize> {
        self.detected_stars
    }

    /// Unsaturated stars with valid measurements.
    #[must_use]
    pub const fn usable_stars(self) -> Option<usize> {
        self.usable_stars
    }

    /// Median major-axis FWHM in pixels.
    #[must_use]
    pub const fn fwhm_major_pixels(self) -> Option<f64> {
        self.fwhm_major_pixels
    }

    /// Median stellar eccentricity.
    #[must_use]
    pub const fn eccentricity(self) -> Option<f64> {
        self.eccentricity
    }
}

/// One frame supplied to a new review book in deterministic processing order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameSpec {
    id: FrameId,
    label: String,
    metrics: FrameMetrics,
}

impl FrameSpec {
    /// Creates a frame row with a user-visible label and optional metrics.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty, oversized, or control-containing
    /// label. Labels may contain paths, but are never used as frame identities.
    pub fn new(
        id: FrameId,
        label: impl Into<String>,
        metrics: FrameMetrics,
    ) -> Result<Self, ReviewError> {
        let label = label.into();
        validate_label(&label)?;
        Ok(Self { id, label, metrics })
    }

    /// Stable frame identity.
    #[must_use]
    pub const fn id(&self) -> &FrameId {
        &self.id
    }

    /// User-visible frame label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Optional quality columns.
    #[must_use]
    pub const fn metrics(&self) -> FrameMetrics {
        self.metrics
    }
}

/// Effective review state of one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// No explicit manual decision exists.
    Undecided,
    /// The frame is explicitly retained.
    Accepted,
    /// The frame is explicitly excluded.
    Rejected,
}

/// Stable reason code for a manual rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualRejectionReason {
    /// Stars or detail are insufficiently sharp.
    Blur,
    /// Tracking or motion elongated the stars.
    Trailing,
    /// Clouds or transparency loss compromised the exposure.
    Cloud,
    /// A satellite, aircraft, or similar trail is unacceptable.
    IntrusiveTrail,
    /// Background gradients are unacceptable.
    Gradient,
    /// Framing or orientation is inconsistent with the set.
    Framing,
    /// Saturation or clipping is unacceptable.
    Saturation,
    /// A defect not covered by a stable predefined reason.
    Other,
}

/// One explicit manual decision, independent of automatic proposals.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManualDecision {
    state: ReviewState,
    rejection_reason: Option<ManualRejectionReason>,
    note: Option<String>,
}

impl ManualDecision {
    /// Creates an explicit acceptance with an optional note.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty, oversized, or unsafe note.
    pub fn accept(note: Option<String>) -> Result<Self, ReviewError> {
        validate_note(note.as_deref())?;
        Ok(Self {
            state: ReviewState::Accepted,
            rejection_reason: None,
            note,
        })
    }

    /// Creates an explicit rejection with a stable reason and optional note.
    ///
    /// `Other` requires a note so the decision never loses its explanation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an invalid note or an unexplained `Other`.
    pub fn reject(
        reason: ManualRejectionReason,
        note: Option<String>,
    ) -> Result<Self, ReviewError> {
        validate_note(note.as_deref())?;
        if reason == ManualRejectionReason::Other && note.is_none() {
            return Err(ReviewError::OtherReasonRequiresNote);
        }
        Ok(Self {
            state: ReviewState::Rejected,
            rejection_reason: Some(reason),
            note,
        })
    }

    /// Decision state; never [`ReviewState::Undecided`].
    #[must_use]
    pub const fn state(&self) -> ReviewState {
        self.state
    }

    /// Stable rejection reason, present only for rejection.
    #[must_use]
    pub const fn rejection_reason(&self) -> Option<ManualRejectionReason> {
        self.rejection_reason
    }

    /// Optional explanatory note.
    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// Requested manual-decision change for one frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionChange {
    frame_id: FrameId,
    decision: Option<ManualDecision>,
}

impl DecisionChange {
    /// Sets or replaces a manual decision.
    #[must_use]
    pub const fn set(frame_id: FrameId, decision: ManualDecision) -> Self {
        Self {
            frame_id,
            decision: Some(decision),
        }
    }

    /// Clears a manual decision, restoring the undecided state.
    #[must_use]
    pub const fn clear(frame_id: FrameId) -> Self {
        Self {
            frame_id,
            decision: None,
        }
    }

    /// Target frame.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Replacement decision, or `None` to clear it.
    #[must_use]
    pub const fn decision(&self) -> Option<&ManualDecision> {
        self.decision.as_ref()
    }
}

/// Column used to order the review table.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    /// Original deterministic processing position.
    ProcessingOrder,
    /// User-visible source label.
    Label,
    /// Explicit review state.
    ReviewState,
    /// Robust background location.
    Background,
    /// Robust background noise.
    Noise,
    /// Detected local maxima.
    DetectedStars,
    /// Valid unsaturated star measurements.
    UsableStars,
    /// Median major-axis FWHM.
    FwhmMajor,
    /// Median stellar eccentricity.
    Eccentricity,
}

/// Direction of a review-table sort.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Lowest values first.
    Ascending,
    /// Highest values first.
    Descending,
}

/// Explicit placement of frames missing the selected metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingPlacement {
    /// Missing values appear before measured values.
    First,
    /// Missing values appear after measured values.
    Last,
}

/// Complete deterministic table-sort request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SortSpec {
    field: SortField,
    direction: SortDirection,
    missing: MissingPlacement,
}

impl SortSpec {
    /// Creates a fully explicit table sort.
    #[must_use]
    pub const fn new(
        field: SortField,
        direction: SortDirection,
        missing: MissingPlacement,
    ) -> Self {
        Self {
            field,
            direction,
            missing,
        }
    }

    /// Selected column.
    #[must_use]
    pub const fn field(self) -> SortField {
        self.field
    }

    /// Selected direction.
    #[must_use]
    pub const fn direction(self) -> SortDirection {
        self.direction
    }

    /// Placement of missing values.
    #[must_use]
    pub const fn missing(self) -> MissingPlacement {
        self.missing
    }
}

#[derive(Clone, Debug)]
struct ReviewEntry {
    spec: FrameSpec,
    decision: Option<ManualDecision>,
}

/// One decision delta exposed before a transaction is applied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionDelta {
    frame_id: FrameId,
    before: Option<ManualDecision>,
    after: Option<ManualDecision>,
}

impl DecisionDelta {
    /// Changed frame.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Decision before the transaction.
    #[must_use]
    pub const fn before(&self) -> Option<&ManualDecision> {
        self.before.as_ref()
    }

    /// Decision after the transaction.
    #[must_use]
    pub const fn after(&self) -> Option<&ManualDecision> {
        self.after.as_ref()
    }
}

/// Immutable, generation-bound preview of a decision transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewPreview {
    base_generation: u64,
    changes: Vec<DecisionDelta>,
}

impl ReviewPreview {
    /// Review generation against which the preview was calculated.
    #[must_use]
    pub const fn base_generation(&self) -> u64 {
        self.base_generation
    }

    /// Effective changes in deterministic request order.
    #[must_use]
    pub fn changes(&self) -> &[DecisionDelta] {
        &self.changes
    }
}

/// Toolkit-independent frame-review state and bounded undo history.
#[derive(Debug)]
pub struct ReviewBook {
    entries: Vec<ReviewEntry>,
    generation: u64,
    sealed: bool,
    undo_history: Vec<Vec<DecisionDelta>>,
    maximum_undo_depth: usize,
}

impl ReviewBook {
    /// Creates a review book whose input order is the immutable processing order.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty or oversized set, duplicate frame
    /// identity, invalid undo bound, or allocation failure.
    pub fn new(frames: Vec<FrameSpec>, maximum_undo_depth: usize) -> Result<Self, ReviewError> {
        if frames.is_empty() {
            return Err(ReviewError::EmptyReviewSet);
        }
        if frames.len() > MAX_REVIEW_FRAMES {
            return Err(ReviewError::TooManyFrames {
                maximum: MAX_REVIEW_FRAMES,
            });
        }
        if maximum_undo_depth == 0 || maximum_undo_depth > MAX_UNDO_DEPTH {
            return Err(ReviewError::InvalidUndoDepth {
                maximum: MAX_UNDO_DEPTH,
            });
        }

        let mut identities = try_vec(frames.len())?;
        identities.extend(frames.iter().map(|frame| frame.id.clone()));
        identities.sort_unstable();
        if identities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReviewError::DuplicateFrameId);
        }

        let mut entries = try_vec(frames.len())?;
        entries.extend(frames.into_iter().map(|spec| ReviewEntry {
            spec,
            decision: None,
        }));
        let mut undo_history = Vec::new();
        undo_history
            .try_reserve_exact(maximum_undo_depth)
            .map_err(|_| ReviewError::AllocationFailed {
                elements: maximum_undo_depth,
            })?;
        Ok(Self {
            entries,
            generation: 0,
            sealed: false,
            undo_history,
            maximum_undo_depth,
        })
    }

    /// Number of frames in the review set.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the review set contains no frames.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Monotonic state generation used to reject stale previews.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether decisions have been sealed for plan construction.
    #[must_use]
    pub const fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Whether one previously applied decision transaction can be undone.
    ///
    /// Sealed books deliberately report `false`, even if their internal
    /// history representation changes in the future, because mutation is no
    /// longer legal once a review plan is being constructed.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.sealed && !self.undo_history.is_empty()
    }

    /// Number of frame decisions affected by the next undo transaction.
    ///
    /// Adapters can use this bound to reserve response storage before the
    /// transaction mutates state. A sealed book exposes no undo operation.
    #[must_use]
    pub fn pending_undo_change_count(&self) -> Option<usize> {
        if self.sealed {
            None
        } else {
            self.undo_history.last().map(Vec::len)
        }
    }

    /// Frame identities in immutable processing order.
    pub fn processing_order(&self) -> impl ExactSizeIterator<Item = &FrameId> {
        self.entries.iter().map(|entry| &entry.spec.id)
    }

    /// User-visible label for one frame.
    #[must_use]
    pub fn label(&self, frame_id: &FrameId) -> Option<&str> {
        self.find_entry(frame_id)
            .map(|entry| entry.spec.label.as_str())
    }

    /// Quality summary for one frame.
    #[must_use]
    pub fn metrics(&self, frame_id: &FrameId) -> Option<FrameMetrics> {
        self.find_entry(frame_id).map(|entry| entry.spec.metrics)
    }

    /// Manual decision for one frame.
    #[must_use]
    pub fn decision(&self, frame_id: &FrameId) -> Option<&ManualDecision> {
        self.find_entry(frame_id)
            .and_then(|entry| entry.decision.as_ref())
    }

    /// Effective state for one frame, or `None` if the identity is unknown.
    #[must_use]
    pub fn state(&self, frame_id: &FrameId) -> Option<ReviewState> {
        self.find_entry(frame_id).map(effective_state)
    }

    /// Calculates a transaction preview without mutating review state.
    ///
    /// Duplicate targets are rejected instead of making request order determine
    /// which change wins. No-op transactions are also explicit errors.
    ///
    /// # Errors
    ///
    /// Returns a typed error for sealed state, invalid batch size, duplicate or
    /// unknown identities, allocation failure, or a transaction with no effect.
    pub fn preview_changes(
        &self,
        changes: &[DecisionChange],
    ) -> Result<ReviewPreview, ReviewError> {
        if self.sealed {
            return Err(ReviewError::Sealed);
        }
        if changes.is_empty() {
            return Err(ReviewError::EmptyBatch);
        }
        if changes.len() > MAX_BATCH_CHANGES {
            return Err(ReviewError::TooManyChanges {
                maximum: MAX_BATCH_CHANGES,
            });
        }

        let mut identities = try_vec(changes.len())?;
        identities.extend(changes.iter().map(|change| change.frame_id.clone()));
        identities.sort_unstable();
        if identities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReviewError::DuplicateChangeTarget);
        }

        let mut resolved = try_vec(changes.len())?;
        for change in changes {
            let Some(entry) = self.find_entry(&change.frame_id) else {
                return Err(ReviewError::UnknownFrame);
            };
            if entry.decision != change.decision {
                resolved.push(DecisionDelta {
                    frame_id: change.frame_id.clone(),
                    before: entry.decision.clone(),
                    after: change.decision.clone(),
                });
            }
        }
        if resolved.is_empty() {
            return Err(ReviewError::NoEffectiveChanges);
        }
        Ok(ReviewPreview {
            base_generation: self.generation,
            changes: resolved,
        })
    }

    /// Applies one previously inspected transaction atomically.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the book is sealed, the preview is stale or
    /// belongs to incompatible state, or the generation counter is exhausted.
    pub fn apply_preview(&mut self, preview: ReviewPreview) -> Result<(), ReviewError> {
        if self.sealed {
            return Err(ReviewError::Sealed);
        }
        if preview.base_generation != self.generation {
            return Err(ReviewError::StalePreview {
                expected: self.generation,
                received: preview.base_generation,
            });
        }
        for change in &preview.changes {
            let Some(entry) = self.find_entry(&change.frame_id) else {
                return Err(ReviewError::IncompatiblePreview);
            };
            if entry.decision != change.before {
                return Err(ReviewError::IncompatiblePreview);
            }
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or(ReviewError::GenerationExhausted)?;

        for change in &preview.changes {
            let Some(entry) = self.find_entry_mut(&change.frame_id) else {
                return Err(ReviewError::IncompatiblePreview);
            };
            entry.decision.clone_from(&change.after);
        }
        if self.undo_history.len() == self.maximum_undo_depth {
            self.undo_history.remove(0);
        }
        self.undo_history.push(preview.changes);
        self.generation = next_generation;
        Ok(())
    }

    /// Reverts the most recently applied transaction as one atomic operation.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the book is sealed, no undo transaction exists,
    /// or the generation counter is exhausted.
    pub fn undo(&mut self) -> Result<(), ReviewError> {
        self.undo_with_deltas().map(drop)
    }

    /// Reverts the most recently applied transaction and returns its inverse.
    ///
    /// The returned deltas describe the transition that was just performed:
    /// `before` is the decision that was active immediately before undo and
    /// `after` is the restored decision. This lets adapters update only the
    /// affected rows without copying the complete review book.
    ///
    /// All fallible allocation and invariant checks happen before the book is
    /// mutated. An error therefore leaves both decisions and history intact.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the book is sealed, no undo transaction exists,
    /// the inverse delta buffer cannot be allocated, an internal identity is no
    /// longer compatible, or the generation counter is exhausted.
    pub fn undo_with_deltas(&mut self) -> Result<Vec<DecisionDelta>, ReviewError> {
        if self.sealed {
            return Err(ReviewError::Sealed);
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or(ReviewError::GenerationExhausted)?;
        let changes = self.undo_history.last().ok_or(ReviewError::NothingToUndo)?;
        let mut entry_indices = try_vec(changes.len())?;
        for change in changes {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| entry.spec.id == change.frame_id)
            else {
                return Err(ReviewError::IncompatiblePreview);
            };
            entry_indices.push(index);
        }
        let mut inverse = try_vec(changes.len())?;
        inverse.extend(changes.iter().map(|change| DecisionDelta {
            frame_id: change.frame_id.clone(),
            before: change.after.clone(),
            after: change.before.clone(),
        }));

        // The history was verified above and no fallible work remains.
        let changes = self.undo_history.pop().ok_or(ReviewError::NothingToUndo)?;
        for (index, change) in entry_indices.into_iter().zip(&changes) {
            self.entries[index].decision.clone_from(&change.before);
        }
        self.generation = next_generation;
        Ok(inverse)
    }

    /// Permanently prevents further decision mutation for plan construction.
    ///
    /// # Errors
    ///
    /// Returns a typed error if already sealed or the generation is exhausted.
    pub fn seal(&mut self) -> Result<(), ReviewError> {
        if self.sealed {
            return Err(ReviewError::Sealed);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(ReviewError::GenerationExhausted)?;
        self.sealed = true;
        self.undo_history.clear();
        Ok(())
    }

    /// Returns frame identities in the requested table order.
    ///
    /// This is a pure view operation. It cannot mutate the processing order or
    /// decisions. Equal measured values use the stable frame identity as their
    /// final tie-breaker.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::AllocationFailed`] when result storage cannot be
    /// reserved.
    pub fn sorted_frame_ids(&self, sort: SortSpec) -> Result<Vec<FrameId>, ReviewError> {
        let mut order = try_vec(self.entries.len())?;
        order.extend(self.entries.iter().enumerate());
        order.sort_by(|(left_index, left), (right_index, right)| {
            compare_entries(*left_index, left, *right_index, right, sort)
                .then_with(|| left.spec.id.cmp(&right.spec.id))
        });
        let mut identities = try_vec(order.len())?;
        identities.extend(order.into_iter().map(|(_, entry)| entry.spec.id.clone()));
        Ok(identities)
    }

    fn find_entry(&self, frame_id: &FrameId) -> Option<&ReviewEntry> {
        self.entries.iter().find(|entry| entry.spec.id == *frame_id)
    }

    fn find_entry_mut(&mut self, frame_id: &FrameId) -> Option<&mut ReviewEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.spec.id == *frame_id)
    }
}

/// Failure to construct or mutate frame-review state safely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewError {
    /// A frame identity was not canonical lowercase SHA-256.
    InvalidFrameId,
    /// Session-relative source path was not bounded and portable.
    InvalidSourcePath,
    /// Source byte length or content fingerprint was invalid.
    InvalidSourceFingerprint,
    /// A frame label was empty, too large, or contained a control character.
    InvalidLabel,
    /// One optional metric violated its finite domain.
    InvalidMetric {
        /// Stable metric field name.
        field: &'static str,
    },
    /// A note was empty, too large, or contained an unsafe control character.
    InvalidNote,
    /// The generic rejection reason requires an explanatory note.
    OtherReasonRequiresNote,
    /// A review set must contain at least one frame.
    EmptyReviewSet,
    /// The review set exceeded its explicit bound.
    TooManyFrames {
        /// Maximum supported frame count.
        maximum: usize,
    },
    /// Two review rows had the same stable identity.
    DuplicateFrameId,
    /// Undo depth must be positive and bounded.
    InvalidUndoDepth {
        /// Maximum supported depth.
        maximum: usize,
    },
    /// A decision batch must not be empty.
    EmptyBatch,
    /// A decision batch exceeded its explicit bound.
    TooManyChanges {
        /// Maximum supported change count.
        maximum: usize,
    },
    /// The same frame appeared more than once in one batch.
    DuplicateChangeTarget,
    /// A requested frame identity was absent.
    UnknownFrame,
    /// Every requested decision already matched current state.
    NoEffectiveChanges,
    /// The preview generation no longer matches current state.
    StalePreview {
        /// Current generation.
        expected: u64,
        /// Generation captured by the preview.
        received: u64,
    },
    /// Preview contents do not match the review set or prior decisions.
    IncompatiblePreview,
    /// No applied decision transaction remains to undo.
    NothingToUndo,
    /// The review book is immutable after sealing.
    Sealed,
    /// The monotonic generation counter cannot advance.
    GenerationExhausted,
    /// A bounded allocation could not be reserved.
    AllocationFailed {
        /// Number of elements requested.
        elements: usize,
    },
}

impl Display for ReviewError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFrameId => formatter
                .write_str("frame identity must be exactly 64 lowercase hexadecimal characters"),
            Self::InvalidSourcePath => {
                formatter.write_str("frame source path must be bounded and portable")
            }
            Self::InvalidSourceFingerprint => formatter.write_str(
                "frame source fingerprint requires a positive length and canonical SHA-256",
            ),
            Self::InvalidLabel => formatter.write_str("frame label is empty, oversized, or unsafe"),
            Self::InvalidMetric { field } => {
                write!(formatter, "review metric `{field}` is invalid")
            }
            Self::InvalidNote => formatter.write_str("review note is empty, oversized, or unsafe"),
            Self::OtherReasonRequiresNote => {
                formatter.write_str("the `other` rejection reason requires a note")
            }
            Self::EmptyReviewSet => {
                formatter.write_str("review set must contain at least one frame")
            }
            Self::TooManyFrames { maximum } => {
                write!(formatter, "review set exceeds the maximum {maximum} frames")
            }
            Self::DuplicateFrameId => {
                formatter.write_str("review set contains a duplicate frame identity")
            }
            Self::InvalidUndoDepth { maximum } => write!(
                formatter,
                "undo depth must be between 1 and the maximum {maximum}"
            ),
            Self::EmptyBatch => formatter.write_str("decision batch must not be empty"),
            Self::TooManyChanges { maximum } => write!(
                formatter,
                "decision batch exceeds the maximum {maximum} changes"
            ),
            Self::DuplicateChangeTarget => {
                formatter.write_str("decision batch targets one frame more than once")
            }
            Self::UnknownFrame => formatter.write_str("decision targets an unknown frame"),
            Self::NoEffectiveChanges => formatter.write_str("decision batch has no effect"),
            Self::StalePreview { expected, received } => write!(
                formatter,
                "decision preview generation {received} is stale; current generation is {expected}"
            ),
            Self::IncompatiblePreview => {
                formatter.write_str("decision preview does not match current review state")
            }
            Self::NothingToUndo => formatter.write_str("no review transaction remains to undo"),
            Self::Sealed => formatter.write_str("review decisions are sealed and immutable"),
            Self::GenerationExhausted => {
                formatter.write_str("review generation counter is exhausted")
            }
            Self::AllocationFailed { elements } => write!(
                formatter,
                "cannot reserve storage for {elements} review elements"
            ),
        }
    }
}

impl Error for ReviewError {}

fn validate_optional_finite(
    field: &'static str,
    value: Option<f64>,
    domain: impl FnOnce(f64) -> bool,
) -> Result<(), ReviewError> {
    if value.is_some_and(|value| !value.is_finite() || !domain(value)) {
        return Err(ReviewError::InvalidMetric { field });
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_portable_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.chars().any(char::is_control)
        && path.split('/').all(valid_path_component)
}

fn valid_path_component(component: &str) -> bool {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.ends_with(' ')
        || component.ends_with('.')
    {
        return false;
    }
    let basename = component
        .split_once('.')
        .map_or(component, |(basename, _)| basename)
        .to_ascii_uppercase();
    !matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !windows_numbered_device(&basename, "COM")
        && !windows_numbered_device(&basename, "LPT")
}

fn windows_numbered_device(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'1'..=b'9']))
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(*byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    encoded
}

fn validate_label(label: &str) -> Result<(), ReviewError> {
    if label.is_empty() || label.len() > MAX_LABEL_BYTES || label.chars().any(char::is_control) {
        return Err(ReviewError::InvalidLabel);
    }
    Ok(())
}

fn validate_note(note: Option<&str>) -> Result<(), ReviewError> {
    let Some(note) = note else {
        return Ok(());
    };
    if note.is_empty()
        || note.len() > MAX_NOTE_BYTES
        || note
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(ReviewError::InvalidNote);
    }
    Ok(())
}

fn effective_state(entry: &ReviewEntry) -> ReviewState {
    entry
        .decision
        .as_ref()
        .map_or(ReviewState::Undecided, ManualDecision::state)
}

fn compare_entries(
    left_index: usize,
    left: &ReviewEntry,
    right_index: usize,
    right: &ReviewEntry,
    sort: SortSpec,
) -> Ordering {
    match sort.field {
        SortField::ProcessingOrder => directed(left_index.cmp(&right_index), sort.direction),
        SortField::Label => directed(left.spec.label.cmp(&right.spec.label), sort.direction),
        SortField::ReviewState => directed(
            state_rank(effective_state(left)).cmp(&state_rank(effective_state(right))),
            sort.direction,
        ),
        SortField::Background => compare_optional_f64(
            left.spec.metrics.background,
            right.spec.metrics.background,
            sort.missing,
            sort.direction,
        ),
        SortField::Noise => compare_optional_f64(
            left.spec.metrics.noise,
            right.spec.metrics.noise,
            sort.missing,
            sort.direction,
        ),
        SortField::DetectedStars => compare_optional_ord(
            left.spec.metrics.detected_stars,
            right.spec.metrics.detected_stars,
            sort.missing,
            sort.direction,
        ),
        SortField::UsableStars => compare_optional_ord(
            left.spec.metrics.usable_stars,
            right.spec.metrics.usable_stars,
            sort.missing,
            sort.direction,
        ),
        SortField::FwhmMajor => compare_optional_f64(
            left.spec.metrics.fwhm_major_pixels,
            right.spec.metrics.fwhm_major_pixels,
            sort.missing,
            sort.direction,
        ),
        SortField::Eccentricity => compare_optional_f64(
            left.spec.metrics.eccentricity,
            right.spec.metrics.eccentricity,
            sort.missing,
            sort.direction,
        ),
    }
}

const fn state_rank(state: ReviewState) -> u8 {
    match state {
        ReviewState::Undecided => 0,
        ReviewState::Accepted => 1,
        ReviewState::Rejected => 2,
    }
}

fn compare_optional_f64(
    left: Option<f64>,
    right: Option<f64>,
    missing: MissingPlacement,
    direction: SortDirection,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => directed(left.total_cmp(&right), direction),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => missing_order(missing),
        (Some(_), None) => missing_order(missing).reverse(),
    }
}

fn compare_optional_ord<T: Ord>(
    left: Option<T>,
    right: Option<T>,
    missing: MissingPlacement,
    direction: SortDirection,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => directed(left.cmp(&right), direction),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => missing_order(missing),
        (Some(_), None) => missing_order(missing).reverse(),
    }
}

const fn missing_order(missing: MissingPlacement) -> Ordering {
    match missing {
        MissingPlacement::First => Ordering::Less,
        MissingPlacement::Last => Ordering::Greater,
    }
}

const fn directed(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn try_vec<T>(elements: usize) -> Result<Vec<T>, ReviewError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| ReviewError::AllocationFailed { elements })?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn metrics(fwhm: Option<f64>) -> TestResult<FrameMetrics> {
        Ok(FrameMetrics::new(
            Some(1_000.0),
            Some(2.0),
            Some(20),
            Some(18),
            fwhm,
            Some(0.2),
        )?)
    }

    fn book() -> TestResult<ReviewBook> {
        Ok(ReviewBook::new(
            vec![
                FrameSpec::new(id('b')?, "second.fits", metrics(Some(3.0))?)?,
                FrameSpec::new(id('a')?, "first.fits", metrics(Some(2.0))?)?,
                FrameSpec::new(id('c')?, "missing.fits", metrics(None)?)?,
            ],
            8,
        )?)
    }

    #[test]
    fn validates_identity_labels_metrics_and_notes() -> TestResult {
        for invalid in ["a", &"A".repeat(64), &"g".repeat(64)] {
            assert_eq!(FrameId::new(invalid), Err(ReviewError::InvalidFrameId));
        }
        assert!(matches!(
            FrameMetrics::new(None, Some(-1.0), None, None, None, None),
            Err(ReviewError::InvalidMetric { field: "noise" })
        ));
        assert!(matches!(
            FrameMetrics::new(None, None, Some(2), Some(3), None, None),
            Err(ReviewError::InvalidMetric {
                field: "usable_stars"
            })
        ));
        assert_eq!(
            FrameSpec::new(id('a')?, "bad\nlabel", FrameMetrics::default()),
            Err(ReviewError::InvalidLabel)
        );
        assert_eq!(
            ManualDecision::reject(ManualRejectionReason::Other, None),
            Err(ReviewError::OtherReasonRequiresNote)
        );
        assert_eq!(
            ManualDecision::accept(Some(String::new())),
            Err(ReviewError::InvalidNote)
        );
        Ok(())
    }

    #[test]
    fn derives_portable_unique_frame_identities() -> TestResult {
        let digest = "a".repeat(64);
        let first = FrameId::derive("lights/frame-001.fits", 1_024, &digest)?;
        let repeated = FrameId::derive("lights/frame-001.fits", 1_024, &digest)?;
        let copied = FrameId::derive("lights/copy-001.fits", 1_024, &digest)?;

        assert_eq!(first, repeated);
        assert_ne!(first, copied);
        assert!(matches!(
            FrameId::derive("../frame.fits", 1_024, &digest),
            Err(ReviewError::InvalidSourcePath)
        ));
        assert!(matches!(
            FrameId::derive("lights/frame.fits", 0, &digest),
            Err(ReviewError::InvalidSourceFingerprint)
        ));
        Ok(())
    }

    #[test]
    fn rejects_empty_duplicate_and_invalid_undo_models() -> TestResult {
        assert!(matches!(
            ReviewBook::new(Vec::new(), 1),
            Err(ReviewError::EmptyReviewSet)
        ));
        let frame = FrameSpec::new(id('a')?, "a.fits", FrameMetrics::default())?;
        assert!(matches!(
            ReviewBook::new(vec![frame.clone(), frame.clone()], 1),
            Err(ReviewError::DuplicateFrameId)
        ));
        assert!(matches!(
            ReviewBook::new(vec![frame], 0),
            Err(ReviewError::InvalidUndoDepth { .. })
        ));
        Ok(())
    }

    #[test]
    fn previews_applies_and_undoes_a_batch_atomically() -> TestResult {
        let mut review = book()?;
        assert!(!review.can_undo());
        let accepted = ManualDecision::accept(None)?;
        let rejected = ManualDecision::reject(
            ManualRejectionReason::Trailing,
            Some("elongated stars".to_owned()),
        )?;
        let changes = [
            DecisionChange::set(id('a')?, accepted.clone()),
            DecisionChange::set(id('b')?, rejected.clone()),
        ];
        let preview = review.preview_changes(&changes)?;

        assert_eq!(preview.base_generation(), 0);
        assert_eq!(preview.changes().len(), 2);
        assert_eq!(review.state(&id('a')?), Some(ReviewState::Undecided));
        review.apply_preview(preview)?;
        assert_eq!(review.state(&id('a')?), Some(ReviewState::Accepted));
        assert_eq!(review.state(&id('b')?), Some(ReviewState::Rejected));
        assert_eq!(review.generation(), 1);
        assert!(review.can_undo());
        assert_eq!(review.pending_undo_change_count(), Some(2));

        let inverse = review.undo_with_deltas()?;
        assert_eq!(inverse.len(), 2);
        assert_eq!(inverse[0].before(), Some(&accepted));
        assert_eq!(inverse[0].after(), None);
        assert_eq!(inverse[1].before(), Some(&rejected));
        assert_eq!(inverse[1].after(), None);
        assert_eq!(review.state(&id('a')?), Some(ReviewState::Undecided));
        assert_eq!(review.state(&id('b')?), Some(ReviewState::Undecided));
        assert_eq!(review.generation(), 2);
        assert!(!review.can_undo());
        assert_eq!(review.pending_undo_change_count(), None);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_noop_unknown_and_stale_changes() -> TestResult {
        let mut review = book()?;
        let accepted = ManualDecision::accept(None)?;
        assert!(matches!(
            review.preview_changes(&[
                DecisionChange::set(id('a')?, accepted.clone()),
                DecisionChange::clear(id('a')?),
            ]),
            Err(ReviewError::DuplicateChangeTarget)
        ));
        assert!(matches!(
            review.preview_changes(&[DecisionChange::clear(id('a')?)]),
            Err(ReviewError::NoEffectiveChanges)
        ));
        assert!(matches!(
            review.preview_changes(&[DecisionChange::set(id('d')?, accepted.clone())]),
            Err(ReviewError::UnknownFrame)
        ));

        let stale = review.preview_changes(&[DecisionChange::set(id('a')?, accepted.clone())])?;
        let current = review.preview_changes(&[DecisionChange::set(id('b')?, accepted)])?;
        review.apply_preview(current)?;
        assert!(matches!(
            review.apply_preview(stale),
            Err(ReviewError::StalePreview {
                expected: 1,
                received: 0,
            })
        ));
        Ok(())
    }

    #[test]
    fn table_sort_never_changes_processing_order() -> TestResult {
        let review = book()?;
        let before: Vec<_> = review.processing_order().cloned().collect();
        let sorted = review.sorted_frame_ids(SortSpec::new(
            SortField::FwhmMajor,
            SortDirection::Ascending,
            MissingPlacement::Last,
        ))?;
        let after: Vec<_> = review.processing_order().cloned().collect();

        assert_eq!(sorted, vec![id('a')?, id('b')?, id('c')?]);
        assert_eq!(before, vec![id('b')?, id('a')?, id('c')?]);
        assert_eq!(after, before);
        Ok(())
    }

    #[test]
    fn equal_metrics_use_stable_identity_and_missing_policy_is_explicit() -> TestResult {
        let review = ReviewBook::new(
            vec![
                FrameSpec::new(id('b')?, "b", metrics(Some(2.0))?)?,
                FrameSpec::new(id('a')?, "a", metrics(Some(2.0))?)?,
                FrameSpec::new(id('c')?, "c", metrics(None)?)?,
            ],
            2,
        )?;
        let first = review.sorted_frame_ids(SortSpec::new(
            SortField::FwhmMajor,
            SortDirection::Ascending,
            MissingPlacement::First,
        ))?;
        assert_eq!(first, vec![id('c')?, id('a')?, id('b')?]);

        let descending_last = review.sorted_frame_ids(SortSpec::new(
            SortField::FwhmMajor,
            SortDirection::Descending,
            MissingPlacement::Last,
        ))?;
        assert_eq!(descending_last, vec![id('a')?, id('b')?, id('c')?]);
        Ok(())
    }

    #[test]
    fn sealing_discards_undo_and_blocks_every_mutation() -> TestResult {
        let mut review = book()?;
        let preview = review
            .preview_changes(&[DecisionChange::set(id('a')?, ManualDecision::accept(None)?)])?;
        review.apply_preview(preview)?;
        review.seal()?;

        assert!(review.is_sealed());
        assert_eq!(review.undo(), Err(ReviewError::Sealed));
        assert_eq!(
            review.preview_changes(&[DecisionChange::clear(id('a')?)]),
            Err(ReviewError::Sealed)
        );
        assert_eq!(review.seal(), Err(ReviewError::Sealed));
        Ok(())
    }

    #[test]
    fn bounded_history_drops_only_the_oldest_transaction() -> TestResult {
        let mut review = ReviewBook::new(
            vec![FrameSpec::new(id('a')?, "a", FrameMetrics::default())?],
            1,
        )?;
        let accept = review
            .preview_changes(&[DecisionChange::set(id('a')?, ManualDecision::accept(None)?)])?;
        review.apply_preview(accept)?;
        let reject = review.preview_changes(&[DecisionChange::set(
            id('a')?,
            ManualDecision::reject(ManualRejectionReason::Blur, None)?,
        )])?;
        review.apply_preview(reject)?;
        review.undo()?;

        assert_eq!(review.state(&id('a')?), Some(ReviewState::Accepted));
        assert_eq!(review.undo(), Err(ReviewError::NothingToUndo));
        Ok(())
    }
}
