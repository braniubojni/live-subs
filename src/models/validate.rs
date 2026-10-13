//! Magic-byte check for a user-picked `.bin` or `.gguf`. Not a model load.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MIN_BYTES: u64 = 1024;

/// Reject an empty, tiny, or non-model file. The error names the problem.
pub fn validate_file(path: &Path) -> Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("{}", path.display()))?;
    if meta.len() == 0 {
        bail!("{}: empty file", path.display());
    }
    if meta.len() < MIN_BYTES {
        bail!("{}: file too small", path.display());
    }
    let mut buf = [0u8; 4];
    let n = File::open(path)?.read(&mut buf)?;
    if n < 4 {
        bail!("{}: file too small", path.display());
    }
    if buf.starts_with(b"GGUF") || buf.starts_with(b"ggml") {
        Ok(())
    } else {
        bail!("{}: not a ggml or GGUF model", path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_files_name_the_problem() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.gguf");
        std::fs::write(&empty, "").unwrap();
        let err = validate_file(&empty).unwrap_err().to_string();
        assert!(err.contains("empty file"), "{err}");

        let short = dir.path().join("short.bin");
        std::fs::write(&short, b"GGUF").unwrap();
        let err = validate_file(&short).unwrap_err().to_string();
        assert!(err.contains("file too small"), "{err}");

        let junk = dir.path().join("junk.bin");
        let mut bytes = vec![0u8; 1024];
        bytes[..4].copy_from_slice(b"nope");
        std::fs::write(&junk, &bytes).unwrap();
        let err = validate_file(&junk).unwrap_err().to_string();
        assert!(err.contains("not a ggml or GGUF model"), "{err}");

        let ok = dir.path().join("ok.gguf");
        let mut bytes = vec![0u8; 1024];
        bytes[..4].copy_from_slice(b"GGUF");
        std::fs::write(&ok, &bytes).unwrap();
        validate_file(&ok).unwrap();

        let ggml = dir.path().join("ok.bin");
        bytes[..4].copy_from_slice(b"ggml");
        std::fs::write(&ggml, &bytes).unwrap();
        validate_file(&ggml).unwrap();
    }
}
