/// Transformation level applied to a value read from the header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Confidence {
    /// The value directly matches its canonical form.
    Exact,
    /// A known case or naming variant was normalized.
    Normalized,
}

/// Canonical value together with its provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalValue<T> {
    value: T,
    source_keyword: String,
    confidence: Confidence,
}

impl<T> CanonicalValue<T> {
    /// Builds a traceable canonical value.
    #[must_use]
    pub fn new(value: T, source_keyword: impl Into<String>, confidence: Confidence) -> Self {
        Self {
            value,
            source_keyword: source_keyword.into(),
            confidence,
        }
    }

    /// Normalized value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// FITS keyword selected as the source.
    #[must_use]
    pub fn source_keyword(&self) -> &str {
        &self.source_keyword
    }

    /// Applied transformation level.
    #[must_use]
    pub const fn confidence(&self) -> Confidence {
        self.confidence
    }
}

/// Sensor type from the calibration pipeline's perspective.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SensorKind {
    /// Color sensor using a color filter array.
    Color,
    /// Monochrome sensor without a color filter array.
    Monochrome,
    /// Unknown type; no demosaicing assumption is allowed.
    Unknown,
}

/// Normalized camera model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CameraModel {
    /// ZWO ASI294MC Pro color camera.
    ZwoAsi294McPro,
    /// ToupTek ATR585C color camera, regardless of its USB transport suffix.
    TouptekAtr585C,
    /// Instrument without a profile, retained under its original name.
    Other(String),
}

impl CameraModel {
    /// Stable name used in sessions and reports.
    #[must_use]
    pub fn canonical_name(&self) -> &str {
        match self {
            Self::ZwoAsi294McPro => "ZWO ASI294MC Pro",
            Self::TouptekAtr585C => "ToupTek ATR585C",
            Self::Other(name) => name,
        }
    }

    /// Known sensor type for this model.
    #[must_use]
    pub const fn sensor_kind(&self) -> SensorKind {
        match self {
            Self::ZwoAsi294McPro | Self::TouptekAtr585C => SensorKind::Color,
            Self::Other(_) => SensorKind::Unknown,
        }
    }

    /// Returns whether the model belongs to the current priority corpus.
    #[must_use]
    pub const fn is_priority_supported(&self) -> bool {
        matches!(self, Self::ZwoAsi294McPro | Self::TouptekAtr585C)
    }
}

/// Scientific type of an acquisition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameType {
    /// Electronic offset or bias frame.
    Bias,
    /// Dark frame.
    Dark,
    /// Flat-field frame.
    Flat,
    /// Scientific light frame.
    Light,
    /// Unrecognized value, retained without interpretation.
    Other(String),
}

/// Color filter array pattern at the image origin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BayerPattern {
    /// Rouge, vert / vert, bleu.
    Rggb,
    /// Bleu, vert / vert, rouge.
    Bggr,
    /// Vert, rouge / bleu, vert.
    Grbg,
    /// Vert, bleu / rouge, vert.
    Gbrg,
    /// Unrecognized pattern, retained as text.
    Other(String),
}

/// Horizontal and vertical binning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binning {
    /// Horizontal factor.
    pub x: u32,
    /// Vertical factor.
    pub y: u32,
}

/// Stable category of a normalization issue.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MetadataIssueCode {
    /// No instrument identifier is present.
    MissingInstrument,
    /// A present keyword contains a value of the wrong type.
    InvalidValue,
    /// Multiple keywords intended to describe one quantity contradict each other.
    ConflictingValues,
}

/// Metadata issue retained for user-facing explanation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataIssue {
    code: MetadataIssueCode,
    keywords: Vec<String>,
    message: String,
}

impl MetadataIssue {
    /// Builds a structured issue.
    #[must_use]
    pub fn new(
        code: MetadataIssueCode,
        keywords: impl IntoIterator<Item = impl Into<String>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            keywords: keywords.into_iter().map(Into::into).collect(),
            message: message.into(),
        }
    }

    /// Stable issue category.
    #[must_use]
    pub const fn code(&self) -> MetadataIssueCode {
        self.code
    }

    /// Involved keywords.
    #[must_use]
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Canonical view of metadata used for grouping and calibration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanonicalMetadata {
    /// Normalized camera.
    pub camera: Option<CanonicalValue<CameraModel>>,
    /// Declared frame type.
    pub frame_type: Option<CanonicalValue<FrameType>>,
    /// Exposure in seconds.
    pub exposure_seconds: Option<CanonicalValue<f64>>,
    /// Measured sensor temperature in degrees Celsius.
    pub sensor_temperature_c: Option<CanonicalValue<f64>>,
    /// Set-point temperature in degrees Celsius.
    pub set_temperature_c: Option<CanonicalValue<f64>>,
    /// Gain as declared by the camera driver.
    pub gain: Option<CanonicalValue<f64>>,
    /// Digital offset as declared by the camera driver.
    pub offset: Option<CanonicalValue<f64>>,
    /// Complete binning, present only when both axes are known.
    pub binning: Option<CanonicalValue<Binning>>,
    /// Declared filter.
    pub filter: Option<CanonicalValue<String>>,
    /// Declared CFA pattern.
    pub bayer_pattern: Option<CanonicalValue<BayerPattern>>,
    /// Issues encountered during normalization.
    pub issues: Vec<MetadataIssue>,
}
