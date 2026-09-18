use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_metadata::{BayerPattern, Binning, CameraModel, CanonicalMetadata, FrameType};

/// Metadata field represented in a strict session grouping key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GroupingField {
    /// Canonical camera model.
    Camera,
    /// Exposure duration.
    Exposure,
    /// Measured sensor temperature.
    SensorTemperature,
    /// Requested sensor temperature.
    SetTemperature,
    /// Camera gain.
    Gain,
    /// Camera digital offset.
    Offset,
    /// Horizontal and vertical binning.
    Binning,
    /// Declared filter.
    Filter,
    /// Declared color filter array pattern.
    BayerPattern,
}

/// Exact, hashable key for conservative session grouping.
///
/// The caller supplies a frame type that has already been resolved through an
/// explicit [`crate::ClassificationPolicy`]. Source keyword names and confidence
/// levels do not affect equality; canonical scientific values do. Finite
/// floating-point values compare by normalized IEEE representation, with `-0.0`
/// and `+0.0` treated as equal.
///
/// Missing metadata remains represented as `None`. Before automatically grouping
/// files, callers must inspect [`Self::missing_fields`] and apply a documented
/// policy appropriate to the frame type and camera profile.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StrictGroupingKey {
    frame_type: FrameType,
    camera: Option<CameraModel>,
    axes: Vec<u64>,
    exposure_seconds: Option<ExactFiniteF64>,
    sensor_temperature_c: Option<ExactFiniteF64>,
    set_temperature_c: Option<ExactFiniteF64>,
    gain: Option<ExactFiniteF64>,
    offset: Option<ExactFiniteF64>,
    binning: Option<Binning>,
    filter: Option<String>,
    bayer_pattern: Option<BayerPattern>,
}

impl StrictGroupingKey {
    /// Builds a strict key from canonical metadata and FITS-order axes.
    ///
    /// # Errors
    ///
    /// Returns an error if a caller-constructed canonical numeric value is NaN
    /// or infinite. The standard normalizer already rejects such values.
    pub fn from_metadata(
        frame_type: FrameType,
        metadata: &CanonicalMetadata,
        axes: &[u64],
    ) -> Result<Self, GroupingKeyError> {
        Ok(Self {
            frame_type,
            camera: metadata.camera.as_ref().map(|value| value.value().clone()),
            axes: axes.to_vec(),
            exposure_seconds: exact_optional(
                metadata
                    .exposure_seconds
                    .as_ref()
                    .map(|value| *value.value()),
                GroupingField::Exposure,
            )?,
            sensor_temperature_c: exact_optional(
                metadata
                    .sensor_temperature_c
                    .as_ref()
                    .map(|value| *value.value()),
                GroupingField::SensorTemperature,
            )?,
            set_temperature_c: exact_optional(
                metadata
                    .set_temperature_c
                    .as_ref()
                    .map(|value| *value.value()),
                GroupingField::SetTemperature,
            )?,
            gain: exact_optional(
                metadata.gain.as_ref().map(|value| *value.value()),
                GroupingField::Gain,
            )?,
            offset: exact_optional(
                metadata.offset.as_ref().map(|value| *value.value()),
                GroupingField::Offset,
            )?,
            binning: metadata.binning.as_ref().map(|value| *value.value()),
            filter: metadata.filter.as_ref().map(|value| value.value().clone()),
            bayer_pattern: metadata
                .bayer_pattern
                .as_ref()
                .map(|value| value.value().clone()),
        })
    }

    /// Resolved scientific frame type.
    #[must_use]
    pub const fn frame_type(&self) -> &FrameType {
        &self.frame_type
    }

    /// Canonical camera model, when declared.
    #[must_use]
    pub const fn camera(&self) -> Option<&CameraModel> {
        self.camera.as_ref()
    }

    /// Image axes in FITS order.
    #[must_use]
    pub fn axes(&self) -> &[u64] {
        &self.axes
    }

    /// Exact exposure duration in seconds, when declared.
    #[must_use]
    pub fn exposure_seconds(&self) -> Option<f64> {
        self.exposure_seconds.map(ExactFiniteF64::value)
    }

    /// Exact measured sensor temperature in degrees Celsius, when declared.
    #[must_use]
    pub fn sensor_temperature_c(&self) -> Option<f64> {
        self.sensor_temperature_c.map(ExactFiniteF64::value)
    }

    /// Exact set-point temperature in degrees Celsius, when declared.
    #[must_use]
    pub fn set_temperature_c(&self) -> Option<f64> {
        self.set_temperature_c.map(ExactFiniteF64::value)
    }

    /// Exact camera gain, when declared.
    #[must_use]
    pub fn gain(&self) -> Option<f64> {
        self.gain.map(ExactFiniteF64::value)
    }

    /// Exact camera digital offset, when declared.
    #[must_use]
    pub fn offset(&self) -> Option<f64> {
        self.offset.map(ExactFiniteF64::value)
    }

    /// Declared binning, when complete.
    #[must_use]
    pub const fn binning(&self) -> Option<Binning> {
        self.binning
    }

    /// Declared filter, when present.
    #[must_use]
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// Declared CFA pattern, when present.
    #[must_use]
    pub const fn bayer_pattern(&self) -> Option<&BayerPattern> {
        self.bayer_pattern.as_ref()
    }

    /// Returns every absent metadata field represented by this key.
    ///
    /// The list is stable and follows the declaration order of [`GroupingField`].
    #[must_use]
    pub fn missing_fields(&self) -> Vec<GroupingField> {
        let mut fields = Vec::new();
        if self.camera.is_none() {
            fields.push(GroupingField::Camera);
        }
        if self.exposure_seconds.is_none() {
            fields.push(GroupingField::Exposure);
        }
        if self.sensor_temperature_c.is_none() {
            fields.push(GroupingField::SensorTemperature);
        }
        if self.set_temperature_c.is_none() {
            fields.push(GroupingField::SetTemperature);
        }
        if self.gain.is_none() {
            fields.push(GroupingField::Gain);
        }
        if self.offset.is_none() {
            fields.push(GroupingField::Offset);
        }
        if self.binning.is_none() {
            fields.push(GroupingField::Binning);
        }
        if self.filter.is_none() {
            fields.push(GroupingField::Filter);
        }
        if self.bayer_pattern.is_none() {
            fields.push(GroupingField::BayerPattern);
        }
        fields
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ExactFiniteF64(u64);

impl ExactFiniteF64 {
    fn new(value: f64, field: GroupingField) -> Result<Self, GroupingKeyError> {
        if !value.is_finite() {
            return Err(GroupingKeyError::NonFiniteValue { field });
        }
        // IEEE considers both zero signs equal. Canonicalizing them prevents an
        // acquisition key from changing because a producer emitted `-0.0`.
        let bits = if value == 0.0 {
            0.0_f64.to_bits()
        } else {
            value.to_bits()
        };
        Ok(Self(bits))
    }

    fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

fn exact_optional(
    value: Option<f64>,
    field: GroupingField,
) -> Result<Option<ExactFiniteF64>, GroupingKeyError> {
    value
        .map(|value| ExactFiniteF64::new(value, field))
        .transpose()
}

/// Error raised while building an exact session grouping key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupingKeyError {
    /// A numeric canonical value is NaN or infinite.
    NonFiniteValue {
        /// Field containing the invalid value.
        field: GroupingField,
    },
}

impl Display for GroupingKeyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteValue { field } => {
                write!(formatter, "grouping field {field:?} must be finite")
            }
        }
    }
}

impl Error for GroupingKeyError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use aether_metadata::{CanonicalValue, Confidence};

    use super::*;

    fn value<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn complete_metadata() -> CanonicalMetadata {
        CanonicalMetadata {
            camera: Some(value(CameraModel::TouptekAtr585C, "INSTRUME")),
            frame_type: Some(value(FrameType::Dark, "IMAGETYP")),
            exposure_seconds: Some(value(60.0, "EXPTIME")),
            sensor_temperature_c: Some(value(-10.0, "CCD-TEMP")),
            set_temperature_c: Some(value(-10.0, "SET-TEMP")),
            gain: Some(value(120.0, "GAIN")),
            offset: Some(value(512.0, "OFFSET")),
            binning: Some(value(Binning { x: 1, y: 1 }, "XBINNING+YBINNING")),
            filter: Some(value("UV/IR Cut".to_owned(), "FILTER")),
            bayer_pattern: Some(value(BayerPattern::Rggb, "BAYERPAT")),
            issues: Vec::new(),
        }
    }

    #[test]
    fn equality_uses_values_not_source_keyword_names() {
        let first = complete_metadata();
        let mut second = complete_metadata();
        second.exposure_seconds = Some(value(60.0, "EXPOSURE"));

        let first_key = StrictGroupingKey::from_metadata(FrameType::Dark, &first, &[3_840, 2_160]);
        let second_key =
            StrictGroupingKey::from_metadata(FrameType::Dark, &second, &[3_840, 2_160]);

        assert_eq!(first_key, second_key);
    }

    #[test]
    fn exact_key_keeps_nearby_temperatures_distinct() {
        let first = complete_metadata();
        let mut second = complete_metadata();
        second.sensor_temperature_c = Some(value(-9.9, "CCD-TEMP"));

        let first_key = StrictGroupingKey::from_metadata(FrameType::Dark, &first, &[3_840, 2_160]);
        let second_key =
            StrictGroupingKey::from_metadata(FrameType::Dark, &second, &[3_840, 2_160]);

        assert_ne!(first_key, second_key);
    }

    #[test]
    fn zero_sign_does_not_split_a_group() {
        let mut first = complete_metadata();
        first.offset = Some(value(-0.0, "OFFSET"));
        let mut second = complete_metadata();
        second.offset = Some(value(0.0, "OFFSET"));

        let first_key = StrictGroupingKey::from_metadata(FrameType::Dark, &first, &[3_840, 2_160]);
        let second_key =
            StrictGroupingKey::from_metadata(FrameType::Dark, &second, &[3_840, 2_160]);

        assert_eq!(first_key, second_key);
    }

    #[test]
    fn key_is_hashable_for_membership_checks() {
        let metadata = complete_metadata();
        let result = StrictGroupingKey::from_metadata(FrameType::Dark, &metadata, &[3_840, 2_160]);
        let Some(key) = result.ok() else {
            return;
        };
        let mut keys = HashSet::new();

        assert!(keys.insert(key.clone()));
        assert!(keys.contains(&key));
    }

    #[test]
    fn rejects_non_finite_caller_constructed_metadata() {
        let mut metadata = complete_metadata();
        metadata.gain = Some(value(f64::NAN, "GAIN"));

        assert_eq!(
            StrictGroupingKey::from_metadata(FrameType::Dark, &metadata, &[3_840, 2_160]),
            Err(GroupingKeyError::NonFiniteValue {
                field: GroupingField::Gain
            })
        );
    }

    #[test]
    fn reports_every_missing_field_without_inventing_values() {
        let metadata = CanonicalMetadata::default();
        let result = StrictGroupingKey::from_metadata(FrameType::Light, &metadata, &[4_144, 2_822]);
        let Some(key) = result.ok() else {
            return;
        };

        assert_eq!(
            key.missing_fields(),
            vec![
                GroupingField::Camera,
                GroupingField::Exposure,
                GroupingField::SensorTemperature,
                GroupingField::SetTemperature,
                GroupingField::Gain,
                GroupingField::Offset,
                GroupingField::Binning,
                GroupingField::Filter,
                GroupingField::BayerPattern,
            ]
        );
    }

    #[test]
    fn axis_order_and_resolved_frame_type_are_part_of_the_key() {
        let metadata = complete_metadata();
        let dark = StrictGroupingKey::from_metadata(FrameType::Dark, &metadata, &[3_840, 2_160]);
        let light = StrictGroupingKey::from_metadata(FrameType::Light, &metadata, &[3_840, 2_160]);
        let transposed =
            StrictGroupingKey::from_metadata(FrameType::Dark, &metadata, &[2_160, 3_840]);

        assert_ne!(dark, light);
        assert_ne!(dark, transposed);
    }
}
