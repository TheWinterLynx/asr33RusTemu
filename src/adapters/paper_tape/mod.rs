//! Filesystem ownership for paper-tape reader and punch.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::core::paper_tape::{PaperTape, PunchMode, PunchState, TapePunch};

pub fn load_reader_file(path: &Path) -> io::Result<PaperTape> {
    fs::read(path).map(PaperTape::new)
}

#[derive(Debug)]
pub struct PunchFile {
    path: PathBuf,
    file: File,
    punch: TapePunch,
}

impl PunchFile {
    pub fn open(path: &Path, mode: PunchMode) -> io::Result<Self> {
        let existing = match mode {
            PunchMode::Append => fs::read(path).or_else(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    Ok(Vec::new())
                } else {
                    Err(error)
                }
            })?,
            PunchMode::Overwrite => Vec::new(),
        };
        let file = match mode {
            PunchMode::Append => OpenOptions::new().create(true).append(true).open(path)?,
            PunchMode::Overwrite => OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?,
        };
        let mut punch = TapePunch::new(mode);
        punch.load(&existing);
        Ok(Self {
            path: path.to_owned(),
            file,
            punch,
        })
    }

    pub fn start(&mut self) -> bool {
        self.punch.start()
    }
    pub fn stop(&mut self) {
        self.punch.stop();
    }
    #[must_use]
    pub fn state(&self) -> PunchState {
        self.punch.state()
    }
    #[must_use]
    pub fn mode(&self) -> PunchMode {
        self.punch.mode()
    }
    pub fn set_mode(&mut self, mode: PunchMode) {
        self.punch.set_mode(mode);
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.punch.bytes().unwrap_or_default()
    }

    pub fn punch(&mut self, data: &[u8]) -> io::Result<bool> {
        if self.punch.state() != PunchState::Running {
            return Ok(false);
        }
        self.file.write_all(data)?;
        if self.punch.punch(data) {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PunchFile, load_reader_file};
    use crate::core::paper_tape::{PunchMode, PunchState};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn reader_loads_exact_binary_bytes() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("reader.pt");
        fs::write(&path, b"\x00A\xff").expect("fixture written");
        assert_eq!(
            load_reader_file(&path).expect("loaded").bytes(),
            b"\x00A\xff"
        );
    }

    #[test]
    fn append_previews_existing_and_appends_in_order() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("append.pt");
        fs::write(&path, b"OLD").expect("fixture written");
        let mut punch = PunchFile::open(&path, PunchMode::Append).expect("opened");
        assert_eq!(punch.bytes(), b"OLD");
        assert_eq!(punch.state(), PunchState::Stopped);
        assert!(!punch.punch(b"OFF").expect("off is harmless"));
        assert!(punch.start());
        assert!(punch.punch(b"NEW").expect("written"));
        drop(punch);
        assert_eq!(fs::read(path).expect("read back"), b"OLDNEW");
    }

    #[test]
    fn overwrite_truncates_immediately_and_mode_change_does_not_reopen() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("overwrite.pt");
        fs::write(&path, b"OLD").expect("fixture written");
        let mut punch = PunchFile::open(&path, PunchMode::Overwrite).expect("opened");
        assert_eq!(fs::metadata(&path).expect("metadata").len(), 0);
        punch.set_mode(PunchMode::Append);
        assert!(punch.start());
        punch.punch(b"NEW").expect("written");
        drop(punch);
        assert_eq!(fs::read(path).expect("read back"), b"NEW");
    }
}
