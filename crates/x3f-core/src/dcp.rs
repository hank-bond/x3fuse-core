//! Read a DCP look table and tone curve without importing camera calibration.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::output::dng::tags;
use crate::output::dng::tiff_writer::{
    DirectoryWriter, Value, TIFF_TYPE_ASCII, TIFF_TYPE_FLOAT, TIFF_TYPE_LONG,
};
use crate::Error;

const MAX_BYTES: usize = 16 * 1024 * 1024;
const PROFILE_MAGIC: u16 = 0x4352;

/// A DCP's look table and tone curve, restricted to its named camera model.
#[derive(Debug)]
pub struct DcpLook {
    name: String,
    camera: String,
    dims: [u32; 3],
    encoding: u32,
    table: Vec<f32>,
    tone: Vec<f32>,
}

impl DcpLook {
    /// Read a DCP file up to 16 MiB in size.
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

    /// Read a little-endian or big-endian extended-profile DCP container.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let directory = Directory::parse(bytes)?;
        let name = directory.text(tags::PROFILE_NAME)?;
        let camera = directory.text(tags::UNIQUE_CAMERA_MODEL)?;
        let policy = directory.optional_long(tags::PROFILE_EMBED_POLICY, 0)?;
        if !matches!(policy, 0 | 1 | 3) {
            return Err(invalid("profile forbids embedding or has unknown policy"));
        }

        let dimensions = directory.field(tags::PROFILE_LOOK_TABLE_DIMS, TIFF_TYPE_LONG, Some(3))?;
        let dims = [
            directory.uint(dimensions.bytes),
            directory.uint(&dimensions.bytes[4..]),
            directory.uint(&dimensions.bytes[8..]),
        ];
        if dims[0] < 1 || dims[1] < 2 || dims[2] < 1 {
            return Err(invalid("invalid look dimensions"));
        }
        let count = dims
            .iter()
            .try_fold(3usize, |count, &dimension| {
                count.checked_mul(dimension as usize)
            })
            .filter(|&count| count <= MAX_BYTES / 4)
            .ok_or_else(|| invalid("look dimensions exceed bounds"))?;
        let encoding = directory.optional_long(tags::PROFILE_LOOK_TABLE_ENCODING, 0)?;
        if encoding > 1 {
            return Err(invalid("unsupported look encoding"));
        }
        let table = directory.floats(tags::PROFILE_LOOK_TABLE_DATA, Some(count))?;
        for (index, entry) in table.as_chunks::<3>().0.iter().enumerate() {
            if !entry.iter().all(|value| value.is_finite()) || entry[1] < 0.0 || entry[2] < 0.0 {
                return Err(invalid(
                    "look values must be finite with nonnegative scales",
                ));
            }
            if index % dims[1] as usize == 0 && entry[2] != 1.0 {
                return Err(invalid("neutral ValueScale must equal one"));
            }
        }
        let tone = directory.floats(tags::PROFILE_TONE_CURVE, None)?;
        if tone.len() < 4
            || tone.len() % 2 != 0
            || tone[..2] != [0.0, 0.0]
            || tone[tone.len() - 2..] != [1.0, 1.0]
            || !tone.iter().all(|value| value.is_finite())
            || tone
                .windows(4)
                .step_by(2)
                .any(|pair| pair[2] <= pair[0] || pair[3] <= pair[1])
        {
            return Err(invalid(
                "tone must be strictly increasing from (0,0) to (1,1)",
            ));
        }
        Ok(Self {
            name,
            camera,
            dims,
            encoding,
            table,
            tone,
        })
    }

    /// Source profile name. Embedding leaves the DNG's profile name unchanged.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// UniqueCameraModel required by the profile.
    pub fn camera(&self) -> &str {
        &self.camera
    }

    pub(crate) fn embed(self, ifd: &mut DirectoryWriter) {
        ifd.add(
            tags::PROFILE_LOOK_TABLE_DIMS,
            Value::Long(self.dims.to_vec()),
        );
        ifd.add(tags::PROFILE_LOOK_TABLE_DATA, Value::Float(self.table));
        ifd.add(
            tags::PROFILE_LOOK_TABLE_ENCODING,
            Value::Long(vec![self.encoding]),
        );
        ifd.add(tags::PROFILE_TONE_CURVE, Value::Float(self.tone));
        ifd.add(
            tags::DNG_BACKWARD_VERSION,
            Value::Byte(tags::DNG_VERSION_1_4_0_0.to_vec()),
        );
    }
}

struct Tag<'a> {
    kind: u16,
    count: usize,
    bytes: &'a [u8],
}

struct Directory<'a> {
    big_endian: bool,
    tags: BTreeMap<u16, Tag<'a>>,
}

impl<'a> Directory<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if !(8..=MAX_BYTES).contains(&bytes.len()) {
            return Err(invalid("file size outside supported bounds"));
        }
        let big_endian = match &bytes[..2] {
            b"MM" => true,
            b"II" => false,
            _ => return Err(invalid("invalid byte order")),
        };
        let short = |data: &[u8]| {
            let word = [data[0], data[1]];
            if big_endian {
                u16::from_be_bytes(word)
            } else {
                u16::from_le_bytes(word)
            }
        };
        if short(&bytes[2..]) != PROFILE_MAGIC {
            return Err(invalid("expected extended-profile DCP header"));
        }
        let mut directory = Self {
            big_endian,
            tags: BTreeMap::new(),
        };
        let offset = directory.uint(&bytes[4..]) as usize;
        if offset < 8 || offset > bytes.len() - 2 {
            return Err(invalid("invalid directory offset"));
        }
        let count = short(&bytes[offset..]) as usize;
        let end = offset + 2 + count * 12;
        if count == 0 || end > bytes.len() {
            return Err(invalid("invalid directory size"));
        }
        for entry in bytes[offset + 2..end].as_chunks::<12>().0 {
            let id = short(entry);
            let kind = short(&entry[2..]);
            let count = directory.uint(&entry[4..]) as usize;
            let size = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Err(invalid("unknown TIFF field type")),
            };
            let length = count
                .checked_mul(size)
                .filter(|&length| length > 0 && length <= bytes.len())
                .ok_or_else(|| invalid("invalid tag size"))?;
            let data = if length <= 4 {
                &entry[8..8 + length]
            } else {
                let start = directory.uint(&entry[8..]) as usize;
                let stop = start
                    .checked_add(length)
                    .filter(|&stop| stop <= bytes.len())
                    .ok_or_else(|| invalid("tag outside file"))?;
                if start < 8 || (stop > offset && start < end) {
                    return Err(invalid("tag overlaps header or directory"));
                }
                &bytes[start..stop]
            };
            if directory
                .tags
                .insert(
                    id,
                    Tag {
                        kind,
                        count,
                        bytes: data,
                    },
                )
                .is_some()
            {
                return Err(invalid("duplicate tag"));
            }
        }
        Ok(directory)
    }

    fn uint(&self, bytes: &[u8]) -> u32 {
        let word = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if self.big_endian {
            u32::from_be_bytes(word)
        } else {
            u32::from_le_bytes(word)
        }
    }

    fn field(&self, id: u16, kind: u16, count: Option<usize>) -> Result<&Tag<'a>, Error> {
        let tag = self
            .tags
            .get(&id)
            .ok_or_else(|| invalid(&format!("missing tag {id}")))?;
        if tag.kind != kind || count.is_some_and(|count| count != tag.count) {
            return Err(invalid(&format!("invalid tag {id}")));
        }
        Ok(tag)
    }

    fn optional_long(&self, id: u16, default: u32) -> Result<u32, Error> {
        if self.tags.contains_key(&id) {
            Ok(self.uint(self.field(id, TIFF_TYPE_LONG, Some(1))?.bytes))
        } else {
            Ok(default)
        }
    }

    fn text(&self, id: u16) -> Result<String, Error> {
        let bytes = self.field(id, TIFF_TYPE_ASCII, None)?.bytes;
        if bytes.len() < 2 || bytes.last() != Some(&0) || bytes[..bytes.len() - 1].contains(&0) {
            return Err(invalid("invalid profile string"));
        }
        std::str::from_utf8(&bytes[..bytes.len() - 1])
            .map(str::to_owned)
            .map_err(|_| invalid("profile text is not UTF-8"))
    }

    fn floats(&self, id: u16, count: Option<usize>) -> Result<Vec<f32>, Error> {
        let tag = self.field(id, TIFF_TYPE_FLOAT, count)?;
        Ok(tag
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_bits(self.uint(bytes)))
            .collect())
    }
}

fn invalid(message: &str) -> Error {
    Error::InvalidData(format!("DCP look: {message}"))
}

#[cfg(test)]
mod tests;
