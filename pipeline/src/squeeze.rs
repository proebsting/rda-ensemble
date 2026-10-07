//! Compressing what a run keeps, and reading it back whatever it is in.
//!
//! An ensemble is enormously redundant: consecutive plans differ by one
//! merge-and-split, so most of each record repeats the one before. That
//! redundancy is at long range, which is why window size decides everything
//! here. On a 100-plan Illinois ensemble of 18.9 MB:
//!
//! | | size | of original |
//! |---|---|---|
//! | gzip -9 | 2,565,657 | 13.58% |
//! | xz -9 | 61,740 | 0.33% |
//! | brotli q11 | 48,109 | 0.25% |
//!
//! gzip's 32 KB window cannot see across plans at all; the others can.
//!
//! # Why xz, when brotli compresses it smaller
//!
//! Because of who has to open it. `lzma` is in the Python standard library
//! and `xzfile()` is base R, so a social scientist can read one of these
//! with nothing installed; 7-Zip opens them on Windows, and `xz` is
//! everywhere else. Brotli needs `pip install brotli` or
//! `install.packages("brotli")`, and has no common desktop tool at all --
//! the brotli command is not even present on the machine this was written
//! on. A fifth of a percent of file size does not buy its way past that.
//!
//! The xz libraries are bindings to C, which would normally be a cost here.
//! It is already paid: rustrecom depends on binary-ensemble, which depends
//! on xz2, so lzma-sys is in the tree whatever this module does. Using the
//! same crate adds nothing. Writing xz in pure Rust is not an option --
//! `lzma-rs` has an encoder, but it emits a valid xz file at 100.00% of the
//! original size.
//!
//! Reading accepts gzip as well, since someone may well have recompressed a
//! file by hand, and uses the multi-stream decoder throughout: a file written
//! across an extension holds two streams, and the single-stream decoder calls
//! that a corrupt file rather than reading on.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use xz2::write::XzEncoder;

/// Compression preset. Six is xz's own default; nine took the same time here
/// and saved 0.001% of the original, which is not worth being different from
/// the number everyone else uses.
const PRESET: u32 = 6;

/// What this writes, and what it recognises on the way back in.
const SUFFIX: &str = ".xz";

/// A writer that compresses as it goes, at `path` plus `.xz`.
///
/// `append` continues an existing file by starting a second xz stream in it
/// rather than trying to resume the first, which is not possible. xz defines
/// a file as a sequence of streams, so the result is an ordinary `.xz` that
/// `xz(1)`, Python's `lzma` and [`open`] below all read as one. It costs a
/// little compression, since the second stream starts with an empty window.
pub fn writer(path: &Path, append: bool) -> Result<Box<dyn Write + Send>> {
    let path = PathBuf::from(format!("{}{SUFFIX}", path.display()));
    let file = if append {
        File::options().append(true).create(true).open(&path)
    } else {
        File::create(&path)
    }
    .with_context(|| format!("opening {}", path.display()))?;
    Ok(Box::new(XzEncoder::new(std::io::BufWriter::new(file), PRESET)))
}

/// Compress `path` in place, leaving `path.xz` and removing the original.
///
/// Returns where it ended up. A file that is already compressed is left
/// alone, so running this twice is harmless.
pub fn compress(path: &Path) -> Result<PathBuf> {
    if is_compressed(path) || !path.exists() {
        return Ok(path.to_path_buf());
    }
    let out_path = PathBuf::from(format!("{}{SUFFIX}", path.display()));
    {
        let mut input = BufReader::new(
            File::open(path).with_context(|| format!("reading {}", path.display()))?,
        );
        let out = File::create(&out_path)
            .with_context(|| format!("creating {}", out_path.display()))?;
        let mut writer = XzEncoder::new(std::io::BufWriter::new(out), PRESET);
        std::io::copy(&mut input, &mut writer)
            .with_context(|| format!("compressing {}", path.display()))?;
        writer.finish()?.flush()?;
    }
    std::fs::remove_file(path)
        .with_context(|| format!("removing {} after compressing it", path.display()))?;
    Ok(out_path)
}

/// The file at `path`, compressed or not, under that name or a compressed
/// one.
///
/// Looks for `path` itself first, then `path.br`, `path.xz`, `path.gz`. The
/// format is decided by what the bytes say rather than by the name, so a
/// file someone renamed still reads.
pub fn open(path: &Path) -> Result<Box<dyn BufRead>> {
    let found = locate(path).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not there, under that name or a compressed one",
            path.display()
        )
    })?;
    let mut file =
        File::open(&found).with_context(|| format!("reading {}", found.display()))?;

    let mut magic = [0u8; 6];
    let n = read_up_to(&mut file, &mut magic)?;
    let head = &magic[..n];
    // Rewind: every decoder below wants the stream from its start.
    let whole = BufReader::new(
        File::open(&found).with_context(|| format!("reading {}", found.display()))?,
    );

    // xz: FD 37 7A 58 5A 00. gzip: 1F 8B.
    //
    // The multi decoder, not the plain one: a file written across an
    // extension holds two streams, and the plain decoder calls that a
    // corrupt stream rather than reading on.
    if head.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        return Ok(Box::new(BufReader::new(
            xz2::read::XzDecoder::new_multi_decoder(whole),
        )));
    }
    if head.starts_with(&[0x1F, 0x8B]) {
        return Ok(Box::new(BufReader::new(flate2::read::GzDecoder::new(whole))));
    }
    Ok(Box::new(whole))
}

/// Whether this name is already one of ours.
pub fn is_compressed(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "xz" || e == "gz")
}

/// Where a file actually is: itself, or a compressed version beside it.
pub fn locate(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    [".xz", ".gz"]
        .iter()
        .map(|ext| PathBuf::from(format!("{}{ext}", path.display())))
        .find(|p| p.exists())
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rda-sq-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// An ensemble repeats itself heavily, which is the whole reason for
    /// compressing it; this stands in for that shape.
    fn repetitive() -> String {
        (0..500)
            .map(|i| format!("{{\"name\":\"{i:06}\",\"plan\":{{\"a\":1,\"b\":2,\"c\":3}}}}\n"))
            .collect()
    }

    #[test]
    fn a_file_survives_the_round_trip() {
        let dir = scratch("round");
        let path = dir.join("plans.jsonl");
        let body = repetitive();
        std::fs::write(&path, &body).unwrap();

        let out = compress(&path).expect("compresses");
        assert_eq!(out, dir.join("plans.jsonl.xz"));
        assert!(!path.exists(), "the uncompressed file is replaced");
        assert!(out.metadata().unwrap().len() < body.len() as u64 / 10);

        // And it is found and read back under its original name.
        let mut back = String::new();
        open(&path).expect("opens").read_to_string(&mut back).unwrap();
        assert_eq!(back, body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_uncompressed_file_reads_unchanged() {
        let dir = scratch("plain");
        let path = dir.join("scores.csv");
        std::fs::write(&path, "name,a\r\n000000,1\r\n").unwrap();
        let mut back = String::new();
        open(&path).expect("opens").read_to_string(&mut back).unwrap();
        assert_eq!(back, "name,a\r\n000000,1\r\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What this writes has to be what the xz everyone else has can read.
    /// That is the entire reason for choosing it.
    #[test]
    fn xz_itself_reads_what_we_wrote() {
        let dir = scratch("interop");
        let path = dir.join("plans.jsonl");
        std::fs::write(&path, repetitive()).unwrap();
        let out = compress(&path).expect("compresses");
        if let Ok(status) = std::process::Command::new("xz").arg("-t").arg(&out).status() {
            assert!(status.success(), "xz(1) rejected the file we wrote");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And someone may well have compressed one by hand.
    #[test]
    fn an_xz_file_reads() {
        let dir = scratch("xz");
        let path = dir.join("plans.jsonl");
        let body = repetitive();
        std::fs::write(&path, &body).unwrap();
        let ok = std::process::Command::new("xz")
            .arg("-9").arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            // xz(1) is not installed here; nothing to assert against.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        assert!(dir.join("plans.jsonl.xz").exists());
        let mut back = String::new();
        open(&path).expect("opens").read_to_string(&mut back).unwrap();
        assert_eq!(back, body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gzip_file_reads() {
        let dir = scratch("gz");
        let path = dir.join("plans.jsonl");
        let body = repetitive();
        std::fs::write(&path, &body).unwrap();
        let ok = std::process::Command::new("gzip")
            .arg("-9").arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let mut back = String::new();
        open(&path).expect("opens").read_to_string(&mut back).unwrap();
        assert_eq!(back, body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compressing_twice_is_harmless() {
        let dir = scratch("twice");
        let path = dir.join("graph.json");
        std::fs::write(&path, "{\"a\":1}").unwrap();
        let once = compress(&path).expect("first");
        let twice = compress(&once).expect("second");
        assert_eq!(once, twice);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_says_so() {
        let dir = scratch("absent");
        let e = match open(&dir.join("nothing.jsonl")) {
            Ok(_) => panic!("a file that is not there must not open"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains("not there"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
