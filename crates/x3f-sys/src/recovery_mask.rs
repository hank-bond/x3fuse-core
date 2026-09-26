//! Read-only native sensor-stress sidecar for training exclusions.
//! PGM: 255 excludes any partially/unreliable layer; 0 is fully healthy.
//! This is not a G1-minus-T1 change mask or a claim of recoverable detail.
use super::{DngCtx, LocalRecovery};
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

thread_local! {
    static OUTPUT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static RESULT: RefCell<Option<io::Result<()>>> = const { RefCell::new(None) };
}

/// Rust-only per-conversion request; never read a process-wide output filename.
pub fn set_output(path: Option<PathBuf>) {
    OUTPUT.with(|slot| *slot.borrow_mut() = path);
    RESULT.with(|slot| *slot.borrow_mut() = None);
}

/// Take immediately after x3f_get_image, like its headroom result.
pub fn take_result() -> Option<io::Result<()>> {
    RESULT.with(|slot| slot.borrow_mut().take())
}

/// Keep request/result local across nested Rayon work. Publish on every exit from
/// x3f_get_image, including early returns, without changing its legacy C signature.
pub(super) struct Export {
    path: Option<PathBuf>,
    result: Option<io::Result<()>>,
}

impl Export {
    pub fn capture() -> Self {
        Self {
            path: OUTPUT.with(|slot| slot.borrow_mut().take()),
            result: None,
        }
    }

    pub fn requested(&self) -> bool {
        self.path.is_some()
    }

    pub fn write(&mut self, ctx: &DngCtx<'_>, model: Option<&LocalRecovery>, bounds: [usize; 4]) {
        let Some(path) = &self.path else {
            return;
        };
        self.result = Some((|| {
            if !ctx.recovery {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "mask export requires DNG recovery",
                ));
            }
            let model = model.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "mask export requires native sensor reliability",
                )
            })?;
            if bounds[0] >= bounds[2]
                || bounds[1] >= bounds[3]
                || bounds[2] > ctx.rows as usize
                || bounds[3] > ctx.cols as usize
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid mask bounds",
                ));
            }
            let excluded = write_file(path, bounds, |r, c| model.mask(r, c))?;
            unsafe {
                crate::x3f_printf(crate::x3f_verbosity_t_DEBUG,
                    c"RECOVERY_MASK_FROZEN bounds=[%zu, %zu, %zu, %zu] excluded=%zu policy=any_layer_below_255\n".as_ptr(),
                    bounds[0], bounds[1], bounds[2], bounds[3], excluded);
            }
            Ok(())
        })());
    }
}

impl Drop for Export {
    fn drop(&mut self) {
        let result = self.path.as_ref().map(|_| {
            self.result.take().unwrap_or_else(|| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "native recovery mask was not generated",
                ))
            })
        });
        RESULT.with(|slot| *slot.borrow_mut() = result);
    }
}

fn write_file(
    path: &Path,
    bounds: [usize; 4],
    mask: impl FnMut(usize, usize) -> [u8; 3],
) -> io::Result<usize> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut out = BufWriter::new(file);
    let excluded = write_mask(&mut out, bounds, mask)?;
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok(excluded)
}

fn write_mask(
    out: &mut impl Write,
    bounds: [usize; 4],
    mut mask: impl FnMut(usize, usize) -> [u8; 3],
) -> io::Result<usize> {
    let [top, left, bottom, right] = bounds;
    assert!(top < bottom && left < right);
    let width = right - left;
    writeln!(out, "P5\n{width} {}\n255", bottom - top)?;
    let mut row = vec![0; width];
    let mut excluded = 0;
    for r in top..bottom {
        for (i, c) in (left..right).enumerate() {
            row[i] = if mask(r, c) == [255; 3] { 0 } else { 255 };
            excluded += usize::from(row[i] != 0);
        }
        out.write_all(&row)?;
    }
    Ok(excluded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_results_survive_nested_conversions_and_clear_on_unrequested_calls() {
        set_output(Some(PathBuf::from("outer.pgm")));
        let mut outer = Export::capture();
        assert!(outer.requested());
        outer.result = Some(Ok(()));
        set_output(Some(PathBuf::from("inner.pgm")));
        drop(Export::capture());
        assert_eq!(
            take_result().unwrap().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        drop(outer);
        assert!(take_result().unwrap().is_ok());
        assert!(take_result().is_none());
        set_output(None);
        let disabled = Export::capture();
        assert!(!disabled.requested());
        drop(disabled);
        assert!(take_result().is_none());
    }

    #[test]
    fn mask_requests_are_thread_local() {
        set_output(Some(PathBuf::from("outer.pgm")));
        std::thread::spawn(|| {
            assert!(Export::capture().path.is_none());
            assert!(take_result().is_none());
        })
        .join()
        .unwrap();
        let request = Export::capture();
        assert_eq!(request.path.as_deref(), Some(Path::new("outer.pgm")));
        drop(request);
        assert!(take_result().unwrap().is_err());
    }

    #[test]
    fn exclusive_mask_creation_preserves_existing_bytes_and_reports_io_errors() {
        let name = format!(
            "x3f-mask-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(name);
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("mask.pgm");
        assert_eq!(
            write_file(&path, [0, 0, 1, 2], |_, c| if c == 0 {
                [255; 3]
            } else {
                [0; 3]
            })
            .unwrap(),
            1
        );
        let expected = b"P5\n2 1\n255\n\x00\xff";
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert_eq!(
            write_file(&path, [0, 0, 1, 1], |_, _| [255; 3])
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert_eq!(
            write_file(
                &directory.join("missing/mask.pgm"),
                [0, 0, 1, 1],
                |_, _| [255; 3]
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::NotFound
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn healthy_partial_and_fully_clipped_are_distinguished() {
        let source = [[255; 3], [254, 255, 255], [255, 0, 255], [0; 3]];
        let mut out = Vec::new();
        assert_eq!(
            write_mask(&mut out, [0, 0, 1, 4], |_, c| source[c]).unwrap(),
            3
        );
        assert_eq!(out, b"P5\n4 1\n255\n\x00\xff\xff\xff");
        assert_eq!(source[0], [255; 3]);
    }

    #[test]
    fn active_bounds_are_cropped_without_rotation_or_coordinate_shift() {
        let mut visited = Vec::new();
        let mut out = Vec::new();
        write_mask(&mut out, [2, 3, 4, 5], |r, c| {
            visited.push((r, c));
            if (r, c) == (3, 4) {
                [255, 254, 255]
            } else {
                [255; 3]
            }
        })
        .unwrap();
        assert_eq!(visited, [(2, 3), (2, 4), (3, 3), (3, 4)]);
        assert_eq!(out, b"P5\n2 2\n255\n\x00\x00\x00\xff");
    }

    #[test]
    fn io_failure_is_not_silently_accepted() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(write_mask(&mut Broken, [0, 0, 1, 1], |_, _| [255; 3]).is_err());
    }
}
