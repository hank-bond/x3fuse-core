use super::*;
use crate::output::dng::tiff_writer::TiffWriter;
use std::ffi::CString;
use std::io::Cursor;

#[path = "../../tests/common/dcp.rs"]
mod fixture;

fn profile() -> Vec<u8> {
    fixture::profile("Test camera", false)
}

fn entry(bytes: &[u8], tag: u16) -> usize {
    (10..10 + u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize * 12)
        .step_by(12)
        .find(|&offset| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) == tag)
        .unwrap()
}

fn set_word(bytes: &mut [u8], tag: u16, field_offset: usize, value: u32) {
    let offset = entry(bytes, tag) + field_offset;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn set_float(bytes: &mut [u8], tag: u16, component: usize, value: f32) {
    let field = entry(bytes, tag) + 8;
    let offset =
        u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize + component * 4;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn omit(bytes: &mut [u8], tag: u16) {
    let offset = entry(bytes, tag);
    bytes[offset..offset + 2].copy_from_slice(&65000u16.to_le_bytes());
}

#[test]
fn both_byte_orders_preserve_nonidentity_values() {
    for big_endian in [false, true] {
        let look = DcpLook::from_bytes(&fixture::profile("Test camera", big_endian)).unwrap();
        assert_eq!(look.camera(), "Test camera");
        assert_eq!(look.name(), "Test look");
        assert_eq!(look.dims, [2, 2, 2]);
        assert_eq!(look.encoding, 1);
        assert_eq!(&look.table[..6], &[0.0, 1.0, 1.0, 12.0, 0.8, 1.1]);
        assert_eq!(&look.table[18..], &[0.0, 1.0, 1.0, -4.0, 1.1, 0.8]);
        assert_eq!(look.tone, [0.0, 0.0, 0.4, 0.6, 1.0, 1.0]);
    }
}

#[test]
fn missing_encoding_defaults_to_linear() {
    let mut bytes = profile();
    omit(&mut bytes, tags::PROFILE_LOOK_TABLE_ENCODING);
    assert_eq!(DcpLook::from_bytes(&bytes).unwrap().encoding, 0);
}

#[test]
fn accepts_supported_embedding_policies_and_missing_policy() {
    for policy in [0, 1, 3] {
        let mut bytes = profile();
        set_word(&mut bytes, tags::PROFILE_EMBED_POLICY, 8, policy);
        DcpLook::from_bytes(&bytes).unwrap();
    }
    let mut bytes = profile();
    omit(&mut bytes, tags::PROFILE_EMBED_POLICY);
    DcpLook::from_bytes(&bytes).unwrap();
}

#[test]
fn rejects_missing_required_fields() {
    for tag in [
        tags::PROFILE_NAME,
        tags::UNIQUE_CAMERA_MODEL,
        tags::PROFILE_TONE_CURVE,
        tags::PROFILE_LOOK_TABLE_DIMS,
        tags::PROFILE_LOOK_TABLE_DATA,
    ] {
        let mut bytes = profile();
        omit(&mut bytes, tag);
        assert!(DcpLook::from_bytes(&bytes).is_err(), "missing tag {tag}");
    }
}

#[test]
fn rejects_bad_headers_offsets_counts_types_and_duplicates() {
    for (offset, data) in [
        (0, vec![0, 0]),
        (2, vec![42, 0]),
        (4, u32::MAX.to_le_bytes().to_vec()),
        (8, vec![255, 255]),
        (12, vec![255, 255]),
        (14, vec![0; 4]),
    ] {
        let mut bytes = profile();
        bytes[offset..offset + data.len()].copy_from_slice(&data);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
    let mut bytes = profile();
    let duplicate = entry(&bytes, tags::PROFILE_NAME);
    bytes[duplicate..duplicate + 2].copy_from_slice(&tags::UNIQUE_CAMERA_MODEL.to_le_bytes());
    assert!(DcpLook::from_bytes(&bytes).is_err());
    for offset in [0, 4, 8, 9, u32::MAX] {
        let mut bytes = profile();
        set_word(&mut bytes, tags::PROFILE_LOOK_TABLE_DATA, 8, offset);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
    for count in [0, 23, 25, u32::MAX] {
        let mut bytes = profile();
        set_word(&mut bytes, tags::PROFILE_LOOK_TABLE_DATA, 4, count);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
}

#[test]
fn rejects_invalid_dimensions() {
    for dims in [[0, 2, 2], [2, 1, 2], [2, 2, 0], [u32::MAX; 3], [2, 2, 3]] {
        let mut bytes = profile();
        let field = entry(&bytes, tags::PROFILE_LOOK_TABLE_DIMS) + 8;
        let offset = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
        for (index, dimension) in dims.iter().enumerate() {
            bytes[offset + index * 4..offset + index * 4 + 4]
                .copy_from_slice(&dimension.to_le_bytes());
        }
        assert!(DcpLook::from_bytes(&bytes).is_err(), "{dims:?}");
    }
}

#[test]
fn rejects_embedding_policy_encoding_and_wrong_scalar_types() {
    for (tag, value) in [
        (tags::PROFILE_EMBED_POLICY, 2),
        (tags::PROFILE_EMBED_POLICY, 99),
        (tags::PROFILE_LOOK_TABLE_ENCODING, 2),
    ] {
        let mut bytes = profile();
        set_word(&mut bytes, tag, 8, value);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
    for tag in [
        tags::PROFILE_EMBED_POLICY,
        tags::PROFILE_LOOK_TABLE_ENCODING,
        tags::PROFILE_LOOK_TABLE_DIMS,
    ] {
        let mut bytes = profile();
        let offset = entry(&bytes, tag) + 2;
        bytes[offset..offset + 2].copy_from_slice(&TIFF_TYPE_FLOAT.to_le_bytes());
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
}

#[test]
fn rejects_invalid_table_and_curve() {
    for (tag, component, value) in [
        (tags::PROFILE_LOOK_TABLE_DATA, 0, f32::NAN),
        (tags::PROFILE_LOOK_TABLE_DATA, 3, f32::INFINITY),
        (tags::PROFILE_LOOK_TABLE_DATA, 1, -1.0),
        (tags::PROFILE_LOOK_TABLE_DATA, 5, -1.0),
        (tags::PROFILE_LOOK_TABLE_DATA, 2, 0.5),
        (tags::PROFILE_TONE_CURVE, 0, 0.1),
        (tags::PROFILE_TONE_CURVE, 2, 0.0),
        (tags::PROFILE_TONE_CURVE, 3, 0.0),
        (tags::PROFILE_TONE_CURVE, 3, f32::INFINITY),
        (tags::PROFILE_TONE_CURVE, 5, 0.9),
    ] {
        let mut bytes = profile();
        set_float(&mut bytes, tag, component, value);
        assert!(
            DcpLook::from_bytes(&bytes).is_err(),
            "{tag}, {component}, {value}"
        );
    }
    for count in [1, 3, 5] {
        let mut bytes = profile();
        set_word(&mut bytes, tags::PROFILE_TONE_CURVE, 4, count);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
}

#[test]
fn rejects_invalid_strings_and_accepts_unicode() {
    let bytes = fixture::profile("カメラ", false);
    assert_eq!(DcpLook::from_bytes(&bytes).unwrap().camera(), "カメラ");
    for replacement in [
        b"\0est camera\0".as_slice(),
        b"Test cameraX",
        b"\xffest camera\0",
    ] {
        let mut bytes = profile();
        let field = entry(&bytes, tags::UNIQUE_CAMERA_MODEL) + 8;
        let offset = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
        bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
        assert!(DcpLook::from_bytes(&bytes).is_err());
    }
}

#[test]
fn unused_calibration_does_not_affect_imported_look() {
    let mut bytes = profile();
    let original = DcpLook::from_bytes(&bytes).unwrap();
    let field = entry(&bytes, tags::COLOR_MATRIX1) + 8;
    let offset = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
    bytes[offset..offset + 4].copy_from_slice(&99999i32.to_le_bytes());
    let changed = DcpLook::from_bytes(&bytes).unwrap();
    assert_eq!(original.table, changed.table);
    assert_eq!(original.tone, changed.tone);
}

fn serialized_directory(ifd: DirectoryWriter) -> Vec<u8> {
    let mut writer = TiffWriter::new(Cursor::new(Vec::new())).unwrap();
    let root = ifd.build(&mut writer).unwrap();
    let mut bytes = writer.finalize(root).unwrap().into_inner();
    bytes[2..4].copy_from_slice(&PROFILE_MAGIC.to_le_bytes());
    bytes
}

#[test]
fn embedding_changes_only_look_fields_and_backward_version() {
    let mut ifd = DirectoryWriter::new();
    let preserved = [
        (
            tags::PROFILE_NAME,
            Value::Ascii(CString::new("Camera default").unwrap()),
        ),
        (
            tags::UNIQUE_CAMERA_MODEL,
            Value::Ascii(CString::new("Test camera").unwrap()),
        ),
        (tags::COLOR_MATRIX1, Value::SRational(vec![(7, 9); 9])),
        (tags::FORWARD_MATRIX1, Value::SRational(vec![(3, 5); 9])),
        (
            tags::AS_SHOT_NEUTRAL,
            Value::Rational(vec![(1, 2), (1, 1), (3, 4)]),
        ),
        (tags::BASELINE_EXPOSURE, Value::SRational(vec![(2, 1)])),
        (
            tags::PROFILE_HUE_SAT_MAP_DATA1,
            Value::Float(vec![0.0, 1.0, 1.0]),
        ),
        (tags::EXTRA_CAMERA_PROFILES, Value::Long(vec![100, 200])),
    ];
    let mut expected = DirectoryWriter::new();
    for (tag, value) in preserved {
        expected.add(tag, value.clone());
        ifd.add(tag, value);
    }
    ifd.add(
        tags::PROFILE_TONE_CURVE,
        Value::Float(vec![0.0, 0.0, 1.0, 1.0]),
    );
    ifd.add(tags::DNG_BACKWARD_VERSION, Value::Byte(vec![1, 3, 0, 0]));
    DcpLook::from_bytes(&profile()).unwrap().embed(&mut ifd);
    let actual_bytes = serialized_directory(ifd);
    let expected_bytes = serialized_directory(expected);
    let actual = Directory::parse(&actual_bytes).unwrap();
    let expected = Directory::parse(&expected_bytes).unwrap();
    assert_eq!(actual.tags.len(), expected.tags.len() + 5);
    for (id, original) in &expected.tags {
        let written = &actual.tags[id];
        assert_eq!(
            (written.kind, written.count, written.bytes),
            (original.kind, original.count, original.bytes)
        );
    }
    let look = DcpLook::from_bytes(&actual_bytes).unwrap();
    let source = DcpLook::from_bytes(&profile()).unwrap();
    assert_eq!(look.name(), "Camera default");
    assert_eq!(look.table, source.table);
    assert_eq!(look.tone, source.tone);
    assert_eq!(
        actual.tags[&tags::DNG_BACKWARD_VERSION].bytes,
        &[1, 4, 0, 0]
    );
}

#[test]
fn truncated_and_mutated_containers_never_panic() {
    for big_endian in [false, true] {
        let bytes = fixture::profile("Test camera", big_endian);
        for end in 0..bytes.len() {
            assert!(std::panic::catch_unwind(|| DcpLook::from_bytes(&bytes[..end])).is_ok());
        }
        for index in 0..bytes.len() {
            let mut modified = bytes.clone();
            modified[index] ^= 0xff;
            assert!(std::panic::catch_unwind(|| DcpLook::from_bytes(&modified)).is_ok());
        }
    }
    assert!(DcpLook::from_bytes(&vec![0; MAX_BYTES + 1]).is_err());
}
