//! Public, generated FITS fixtures for the priority color-camera profiles.
//!
//! The fixtures deliberately use a 4 x 3 sensor and invented acquisition
//! values. Only the format and metadata semantics resemble supported inputs;
//! no source image, target name, coordinates, or observer data is embedded.

use std::io::Cursor;

use aether_fits::{BLOCK_SIZE, CARD_SIZE, HeaderReadOptions, ImageRegion, PrimaryImageReader};
use aether_metadata::{
    BayerPattern, Binning, CameraModel, CanonicalValue, Confidence, FrameType, normalize_header,
};

const WIDTH: usize = 4;
const HEIGHT: usize = 3;
const STORED_PIXELS: [i16; WIDTH * HEIGHT] = [
    i16::MIN,
    -16_384,
    -1,
    0,
    1,
    7,
    42,
    1_024,
    4_096,
    16_384,
    30_000,
    i16::MAX,
];

fn fixed_card(keyword: &str, value: &str) -> String {
    format!("{keyword:<8}= {value:>20}")
}

fn string_card(keyword: &str, value: &str) -> String {
    format!("{keyword:<8}= '{value}'")
}

fn priority_camera_fixture(instrument: &str) -> Vec<u8> {
    let cards = [
        fixed_card("SIMPLE", "T"),
        fixed_card("BITPIX", "16"),
        fixed_card("NAXIS", "2"),
        fixed_card("NAXIS1", &WIDTH.to_string()),
        fixed_card("NAXIS2", &HEIGHT.to_string()),
        fixed_card("BSCALE", "1"),
        fixed_card("BZERO", "32768"),
        string_card("INSTRUME", instrument),
        string_card("IMAGETYP", "Light"),
        fixed_card("EXPTIME", "12.5"),
        fixed_card("CCD-TEMP", "-7.0"),
        fixed_card("SET-TEMP", "-10.0"),
        fixed_card("GAIN", "42"),
        fixed_card("OFFSET", "7"),
        fixed_card("XBINNING", "1"),
        fixed_card("YBINNING", "1"),
        string_card("FILTER", "SYNTHETIC"),
        string_card("BAYERPAT", "RGGB"),
        "END".to_owned(),
    ];

    let mut bytes = vec![b' '; BLOCK_SIZE];
    for (index, card) in cards.iter().enumerate() {
        let start = index * CARD_SIZE;
        let end = start + CARD_SIZE;
        let Some(destination) = bytes.get_mut(start..end) else {
            return Vec::new();
        };
        let source = card.as_bytes();
        let length = source.len().min(CARD_SIZE);
        destination[..length].copy_from_slice(&source[..length]);
    }

    bytes.extend(STORED_PIXELS.iter().flat_map(|value| value.to_be_bytes()));
    bytes.resize(bytes.len().next_multiple_of(BLOCK_SIZE), 0);
    bytes
}

fn assert_priority_camera_fixture(
    instrument: &str,
    expected_camera: CameraModel,
    expected_confidence: Confidence,
) {
    let input = priority_camera_fixture(instrument);
    assert!(!input.is_empty());
    assert_eq!(input.len() % BLOCK_SIZE, 0);

    let result = PrimaryImageReader::open(Cursor::new(input), HeaderReadOptions::default());
    assert!(result.is_ok());
    let Some(mut reader) = result.ok() else {
        return;
    };
    assert!(reader.report().is_conformant());
    assert_eq!(reader.descriptor().axes(), &[WIDTH as u64, HEIGHT as u64]);

    let metadata = normalize_header(reader.report().header());
    assert!(metadata.issues.is_empty());
    assert_eq!(
        metadata.camera.as_ref().map(CanonicalValue::value),
        Some(&expected_camera)
    );
    assert_eq!(
        metadata.camera.as_ref().map(CanonicalValue::confidence),
        Some(expected_confidence)
    );
    assert_eq!(
        metadata.frame_type.as_ref().map(CanonicalValue::value),
        Some(&FrameType::Light)
    );
    assert_eq!(
        metadata.frame_type.as_ref().map(CanonicalValue::confidence),
        Some(Confidence::Normalized)
    );
    assert_eq!(
        metadata.bayer_pattern.as_ref().map(CanonicalValue::value),
        Some(&BayerPattern::Rggb)
    );
    assert_eq!(
        metadata.binning.as_ref().map(CanonicalValue::value),
        Some(&Binning { x: 1, y: 1 })
    );
    assert_eq!(
        metadata
            .exposure_seconds
            .as_ref()
            .map(CanonicalValue::value),
        Some(&12.5)
    );
    assert_eq!(
        metadata
            .sensor_temperature_c
            .as_ref()
            .map(CanonicalValue::value),
        Some(&-7.0)
    );
    assert_eq!(
        metadata.gain.as_ref().map(CanonicalValue::value),
        Some(&42.0)
    );
    assert_eq!(
        metadata.offset.as_ref().map(CanonicalValue::value),
        Some(&7.0)
    );

    let result = reader.read_region_image(ImageRegion::new(0, 0, 0, WIDTH as u64, HEIGHT as u64));
    assert!(result.is_ok());
    let Some(image) = result.ok() else {
        return;
    };
    let expected: Vec<u64> = STORED_PIXELS
        .iter()
        .map(|value| (f64::from(*value) + 32_768.0).to_bits())
        .collect();
    let actual: Vec<u64> = image.pixels().iter().map(|value| value.to_bits()).collect();

    assert_eq!(actual, expected);
    assert!(image.mask().as_slice().iter().all(|flags| flags.is_clear()));
}

#[test]
fn reads_synthetic_zwo_asi294mc_pro_fixture() {
    assert_priority_camera_fixture(
        "ZWO ASI294MC Pro",
        CameraModel::ZwoAsi294McPro,
        Confidence::Exact,
    );
}

#[test]
fn reads_synthetic_touptek_atr585c_fixture() {
    assert_priority_camera_fixture(
        "ATR585C(USB2.0)",
        CameraModel::TouptekAtr585C,
        Confidence::Normalized,
    );
}
