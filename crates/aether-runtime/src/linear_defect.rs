use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_calibration::{
    LinearDefectAxis, LinearDefectCorrectionParameters, LinearDefectDetectionParameters,
};
use sha2::{Digest, Sha256};

const PARAMETER_DOMAIN: &[u8] = b"aetherstack-linear-defect-parameters-v1\0";

/// Failure to construct one canonical linear-defect parameter identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinearDefectParameterSealError {
    /// Detection and correction target different line orientations.
    AxisMismatch,
    /// Detection and correction use different detector lattices.
    StrideMismatch,
}

impl Display for LinearDefectParameterSealError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AxisMismatch => {
                formatter.write_str("linear-defect detection and correction axes differ")
            }
            Self::StrideMismatch => {
                formatter.write_str("linear-defect detection and correction strides differ")
            }
        }
    }
}

impl Error for LinearDefectParameterSealError {}

/// Returns a path-free identity for every line-detection and repair control.
pub fn strict_linear_defect_parameters_sha256(
    detection: LinearDefectDetectionParameters,
    correction: LinearDefectCorrectionParameters,
) -> Result<String, LinearDefectParameterSealError> {
    if detection.axis() != correction.axis() {
        return Err(LinearDefectParameterSealError::AxisMismatch);
    }
    if detection.stride() != correction.stride() {
        return Err(LinearDefectParameterSealError::StrideMismatch);
    }
    let mut digest = Sha256::new();
    digest.update(PARAMETER_DOMAIN);
    digest.update([axis_tag(detection.axis())]);
    update_usize(&mut digest, detection.perpendicular_radius());
    update_usize(&mut digest, detection.stride());
    update_usize(&mut digest, detection.minimum_perpendicular_neighbours());
    update_usize(&mut digest, detection.minimum_affected_samples());
    digest.update(detection.minimum_affected_fraction_ppm().to_be_bytes());
    digest.update(detection.hot_sigma().to_bits().to_be_bytes());
    digest.update(detection.cold_sigma().to_bits().to_be_bytes());
    digest.update(
        detection
            .minimum_absolute_deviation()
            .to_bits()
            .to_be_bytes(),
    );
    update_usize(&mut digest, correction.perpendicular_radius());
    update_usize(&mut digest, correction.minimum_perpendicular_neighbours());
    Ok(lowercase_hex(&digest.finalize()))
}

const fn axis_tag(axis: LinearDefectAxis) -> u8 {
    match axis {
        LinearDefectAxis::Rows => 0,
        LinearDefectAxis::Columns => 1,
    }
}

fn update_usize(digest: &mut Sha256, value: usize) {
    digest.update((value as u128).to_be_bytes());
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    type Policies = (
        LinearDefectDetectionParameters,
        LinearDefectCorrectionParameters,
    );

    fn policies(axis: LinearDefectAxis, stride: usize) -> Result<Policies, Box<dyn Error>> {
        Ok((
            LinearDefectDetectionParameters::new(axis, 2, stride, 3, 16, 500_000, 5.0, 4.0, -0.0)?,
            LinearDefectCorrectionParameters::new(axis, 3, stride, 4)?,
        ))
    }

    #[test]
    fn seal_is_stable_path_free_and_binds_every_policy_family() -> Result<(), Box<dyn Error>> {
        let (detection, correction) = policies(LinearDefectAxis::Rows, 2)?;
        let first = strict_linear_defect_parameters_sha256(detection, correction)?;
        let second = strict_linear_defect_parameters_sha256(detection, correction)?;
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );

        let (columns, column_correction) = policies(LinearDefectAxis::Columns, 2)?;
        assert_ne!(
            first,
            strict_linear_defect_parameters_sha256(columns, column_correction)?
        );
        let changed = LinearDefectCorrectionParameters::new(LinearDefectAxis::Rows, 4, 2, 4)?;
        assert_ne!(
            first,
            strict_linear_defect_parameters_sha256(detection, changed)?
        );
        Ok(())
    }

    #[test]
    fn seal_rejects_mixed_axis_and_lattice_policies() -> Result<(), Box<dyn Error>> {
        let (detection, correction) = policies(LinearDefectAxis::Rows, 1)?;
        let columns = LinearDefectCorrectionParameters::new(LinearDefectAxis::Columns, 3, 1, 4)?;
        assert_eq!(
            strict_linear_defect_parameters_sha256(detection, columns),
            Err(LinearDefectParameterSealError::AxisMismatch)
        );
        let stride_two = LinearDefectCorrectionParameters::new(LinearDefectAxis::Rows, 3, 2, 4)?;
        assert_eq!(
            strict_linear_defect_parameters_sha256(detection, stride_two),
            Err(LinearDefectParameterSealError::StrideMismatch)
        );
        assert!(strict_linear_defect_parameters_sha256(detection, correction).is_ok());
        Ok(())
    }
}
