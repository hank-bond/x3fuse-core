//! Selective DCP look import, not camera-profile replacement.
//!
//! Imports ProfileLookTableDims/Data/Encoding and ProfileToneCurve only. Camera
//! matrices, white balance, exposure and calibration remain those of the DNG.
//! Supports a bounded classic extended-profile container in either byte order,
//! with a complete look table and strictly increasing endpoint-normalized curve.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::output::dng::tiff_writer::{DirectoryWriter, Value};
use crate::Error;

const MAX_BYTES: usize = 16 * 1024 * 1024;

fn invalid(message: &str) -> Error {
    Error::InvalidData(format!("DCP look: {message}"))
}

/// Validated look data. Other DCP profile fields are deliberately not retained.
#[derive(Debug)]
pub struct DcpLook {
    name: String,
    camera: String,
    dims: [u32; 3],
    encoding: u32,
    table: Vec<f32>,
    tone: Vec<f32>,
}

struct Tag<'a> {
    kind: u16,
    count: usize,
    bytes: &'a [u8],
}

impl DcpLook {
    /// Read at most 16 MiB. No DNG or X3F data is changed by loading a look.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let io_error = |source| Error::Io {
            path: path.display().to_string(),
            source,
        };
        if !std::fs::metadata(path).map_err(io_error)?.is_file() {
            return Err(invalid("expected a regular profile file"));
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(io_error)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Self::from_bytes(&bytes)
    }

    /// Parse the selected DCP fields, rejecting malformed ranges and duplicate tags.
    /// Matrix values in the container are never used to interpret the raw image.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if !(8..=MAX_BYTES).contains(&bytes.len()) {
            return Err(invalid("file size outside supported bounds"));
        }
        let be = match &bytes[..2] {
            b"MM" => true,
            b"II" => false,
            _ => return Err(invalid("invalid byte order")),
        };
        let u16_at = |p: usize| -> u16 {
            let a = [bytes[p], bytes[p + 1]];
            if be {
                u16::from_be_bytes(a)
            } else {
                u16::from_le_bytes(a)
            }
        };
        let uint = |b: &[u8]| -> u32 {
            let a = [b[0], b[1], b[2], b[3]];
            if be {
                u32::from_be_bytes(a)
            } else {
                u32::from_le_bytes(a)
            }
        };
        if u16_at(2) != 0x4352 {
            return Err(invalid("expected extended-profile DCP header"));
        }
        let offset = uint(&bytes[4..8]) as usize;
        if offset < 8 || offset > bytes.len() - 2 {
            return Err(invalid("invalid directory offset"));
        }
        let n = u16_at(offset) as usize;
        let end = offset + 2 + n * 12;
        if n == 0 || n > 4096 || end > bytes.len() {
            return Err(invalid("invalid directory size"));
        }
        let mut tags = BTreeMap::new();
        for i in 0..n {
            let p = offset + 2 + i * 12;
            let id = u16_at(p);
            let kind = u16_at(p + 2);
            let count = uint(&bytes[p + 4..p + 8]) as usize;
            let size = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Err(invalid("unknown TIFF field type")),
            };
            let length = count
                .checked_mul(size)
                .ok_or_else(|| invalid("tag size overflow"))?;
            if count == 0 || length > MAX_BYTES {
                return Err(invalid("invalid tag size"));
            }
            let start = if length <= 4 {
                p + 8
            } else {
                uint(&bytes[p + 8..p + 12]) as usize
            };
            let stop = start
                .checked_add(length)
                .ok_or_else(|| invalid("tag range overflow"))?;
            if stop > bytes.len() || (length > 4 && (start < 8 || (stop > offset && start < end))) {
                return Err(invalid("tag outside file or overlapping directory"));
            }
            if tags
                .insert(
                    id,
                    Tag {
                        kind,
                        count,
                        bytes: &bytes[start..stop],
                    },
                )
                .is_some()
            {
                return Err(invalid("duplicate tag"));
            }
        }
        let field = |id: u16, kind: u16, count: Option<usize>| -> Result<&Tag<'_>, Error> {
            let tag = tags
                .get(&id)
                .ok_or_else(|| invalid(&format!("missing tag {id}")))?;
            if tag.kind != kind || count.is_some_and(|n| n != tag.count) {
                return Err(invalid(&format!("invalid tag {id}")));
            }
            Ok(tag)
        };
        let text = |id| -> Result<String, Error> {
            let b = field(id, 2, None)?.bytes;
            if b.len() < 2 || b.last() != Some(&0) || b[..b.len() - 1].contains(&0) {
                return Err(invalid("invalid profile string"));
            }
            String::from_utf8(b[..b.len() - 1].to_vec())
                .map_err(|_| invalid("profile text is not UTF-8"))
        };
        if tags.contains_key(&50941) && !matches!(uint(field(50941, 4, Some(1))?.bytes), 0 | 1 | 3)
        {
            return Err(invalid("profile forbids embedding or has unknown policy"));
        }
        let d = field(50981, 4, Some(3))?.bytes;
        let dims = [uint(d), uint(&d[4..]), uint(&d[8..])];
        let cells = dims
            .iter()
            .try_fold(1usize, |n, &d| n.checked_mul(d as usize));
        let count = cells
            .and_then(|n| n.checked_mul(3))
            .filter(|&n| n <= MAX_BYTES / 4)
            .ok_or_else(|| invalid("look dimensions exceed bounds"))?;
        if dims[0] < 1 || dims[1] < 2 || dims[2] < 1 {
            return Err(invalid("invalid look dimensions"));
        }
        let encoding = if tags.contains_key(&51108) {
            uint(field(51108, 4, Some(1))?.bytes)
        } else {
            0
        };
        if encoding > 1 {
            return Err(invalid("unsupported look encoding"));
        }
        let floats = |tag: &Tag<'_>| -> Vec<f32> {
            tag.bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_bits(uint(b)))
                .collect()
        };
        let table = floats(field(50982, 11, Some(count))?);
        for (i, entry) in table.as_chunks::<3>().0.iter().enumerate() {
            if !entry.iter().all(|v| v.is_finite())
                || entry[1] < 0.0
                || entry[2] < 0.0
                || (i % dims[1] as usize == 0 && entry[2] != 1.0)
            {
                return Err(invalid("invalid look values or neutral ValueScale"));
            }
        }
        let tone = floats(field(50940, 11, None)?);
        if tone.len() < 4
            || tone.len() % 2 != 0
            || tone[..2] != [0.0, 0.0]
            || tone[tone.len() - 2..] != [1.0, 1.0]
            || !tone.iter().all(|v| v.is_finite())
            || tone
                .windows(4)
                .step_by(2)
                .any(|w| w[2] <= w[0] || w[3] <= w[1])
        {
            return Err(invalid(
                "tone must be strictly increasing from (0,0) to (1,1)",
            ));
        }
        Ok(Self {
            name: text(50936)?,
            camera: text(50708)?,
            dims,
            encoding,
            table,
            tone,
        })
    }

    /// Informational name of the source look; the DNG's camera-profile name stays intact.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact UniqueCameraModel required by this look package.
    pub fn camera(&self) -> &str {
        &self.camera
    }

    pub(crate) fn embed(self, ifd: &mut DirectoryWriter) {
        // This whitelist is the calibration boundary. Never copy other DCP tags.
        ifd.add(50981, Value::Long(self.dims.to_vec()));
        ifd.add(50982, Value::Float(self.table));
        ifd.add(51108, Value::Long(vec![self.encoding]));
        ifd.add(50940, Value::Float(self.tone));
        ifd.add(50707, Value::Byte(vec![1, 4, 0, 0]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(be: bool) -> Vec<u8> {
        let u16b = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32b = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let words = |v: &[u32]| v.iter().flat_map(|&n| u32b(n)).collect::<Vec<_>>();
        let mut table = Vec::new();
        for _ in 0..8 {
            table.extend([0f32.to_bits(), 1f32.to_bits(), 1f32.to_bits()]);
        }
        let fields = [
            (50708u16, 2u16, b"Test camera\0".to_vec()),
            (50936, 2, b"Test look\0".to_vec()),
            (
                50940,
                11,
                words(&[
                    0f32.to_bits(),
                    0f32.to_bits(),
                    1f32.to_bits(),
                    1f32.to_bits(),
                ]),
            ),
            (50941, 4, words(&[0])),
            (50981, 4, words(&[2, 2, 2])),
            (50982, 11, words(&table)),
            (51108, 4, words(&[1])),
            // A calibration value is present, but must not enter the look.
            (
                50721,
                10,
                words(&[123, 100, 0, 1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1]),
            ),
        ];
        let mut bytes = Vec::from(if be { b"MM" } else { b"II" });
        bytes.extend(u16b(0x4352));
        bytes.extend(u32b(8));
        bytes.extend(u16b(fields.len() as u16));
        bytes.resize(10 + fields.len() * 12 + 4, 0);
        for (i, (id, kind, value)) in fields.iter().enumerate() {
            let p = 10 + i * 12;
            bytes[p..p + 2].copy_from_slice(&u16b(*id));
            bytes[p + 2..p + 4].copy_from_slice(&u16b(*kind));
            let size = match kind {
                2 => 1,
                10 => 8,
                _ => 4,
            };
            bytes[p + 4..p + 8].copy_from_slice(&u32b((value.len() / size) as u32));
            if value.len() <= 4 {
                bytes[p + 8..p + 8 + value.len()].copy_from_slice(value);
            } else {
                let offset = bytes.len() as u32;
                bytes[p + 8..p + 12].copy_from_slice(&u32b(offset));
                bytes.extend(value);
            }
        }
        bytes
    }

    fn entry(bytes: &mut [u8], index: usize, offset: usize, value: u32) {
        let p = 10 + index * 12 + offset;
        bytes[p..p + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn both_byte_orders_preserve_look_values() {
        for be in [false, true] {
            let p = DcpLook::from_bytes(&fixture(be)).unwrap();
            assert_eq!(p.camera(), "Test camera");
            assert_eq!(p.name(), "Test look");
            assert_eq!(p.dims, [2, 2, 2]);
            assert_eq!(p.encoding, 1);
            assert_eq!(p.table, [0.0, 1.0, 1.0].repeat(8));
            assert_eq!(p.tone, [0.0, 0.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn rejects_bad_headers_offsets_counts_types_and_duplicates() {
        for (p, data) in [
            (0, vec![0, 0]),
            (2, vec![42, 0]),
            (4, u32::MAX.to_le_bytes().to_vec()),
            (8, vec![255, 255]),
            (12, vec![255, 255]),
            (14, vec![0; 4]),
        ] {
            let mut b = fixture(false);
            b[p..p + data.len()].copy_from_slice(&data);
            assert!(DcpLook::from_bytes(&b).is_err());
        }
        let mut b = fixture(false);
        let id = b[10..12].to_vec();
        b[22..24].copy_from_slice(&id);
        assert!(DcpLook::from_bytes(&b).is_err());
        let mut b = fixture(false);
        entry(&mut b, 5, 8, 8); // value in directory
        assert!(DcpLook::from_bytes(&b).is_err());
        let mut b = fixture(false);
        entry(&mut b, 5, 4, u32::MAX);
        assert!(DcpLook::from_bytes(&b).is_err());
    }

    #[test]
    fn rejects_embedding_policy_and_encoding() {
        for (index, value) in [(3, 2), (3, 99), (6, 2)] {
            let mut b = fixture(false);
            entry(&mut b, index, 8, value);
            assert!(DcpLook::from_bytes(&b).is_err());
        }
    }

    #[test]
    fn rejects_invalid_table_and_curve() {
        for (index, component, value) in [
            (5, 0, f32::NAN),
            (5, 1, -1.0),
            (5, 2, 0.5),
            (2, 2, 0.0),
            (2, 3, f32::INFINITY),
        ] {
            let mut b = fixture(false);
            let p = 10 + index * 12 + 8;
            let offset =
                u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize + component * 4;
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(DcpLook::from_bytes(&b).is_err());
        }
    }

    #[test]
    fn unused_calibration_does_not_affect_imported_look() {
        let mut b = fixture(false);
        let original = DcpLook::from_bytes(&b).unwrap();
        let offset = u32::from_le_bytes(b[102..106].try_into().unwrap()) as usize;
        b[offset..offset + 4].copy_from_slice(&99999i32.to_le_bytes());
        let changed = DcpLook::from_bytes(&b).unwrap();
        assert_eq!(original.table, changed.table);
        assert_eq!(original.tone, changed.tone);
    }

    #[test]
    fn truncated_and_mutated_containers_never_panic() {
        let b = fixture(false);
        for end in 0..b.len() {
            assert!(std::panic::catch_unwind(|| DcpLook::from_bytes(&b[..end])).is_ok());
        }
        for i in 0..b.len() {
            let mut modified = b.clone();
            modified[i] ^= 0xff;
            assert!(std::panic::catch_unwind(|| DcpLook::from_bytes(&modified)).is_ok());
        }
        assert!(DcpLook::from_bytes(&vec![0; MAX_BYTES + 1]).is_err());
    }
}
