use crate::{Error, Result};
use std::{fs::OpenOptions, io::Write, path::{Path, PathBuf}};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachment {
    pub section: String,
    pub filename: String,
    pub mime_type: String,
    /// Encoded octets reported by BODYSTRUCTURE.
    pub size: u32,
    pub encoding: String,
}

pub(crate) fn decode(bytes: &[u8], encoding: &str) -> Result<Vec<u8>> {
    let encoding = match encoding.to_ascii_lowercase().as_str() {
        "base64" => mail_parser::Encoding::Base64,
        "quoted-printable" => mail_parser::Encoding::QuotedPrintable,
        "7bit" | "8bit" | "binary" => mail_parser::Encoding::None,
        _ => return Err(Error::Invalid("unsupported attachment encoding")),
    };
    let (bytes, malformed) = encoding.decode_checked(bytes);
    if malformed {
        return Err(Error::Invalid("invalid attachment encoding"));
    }
    Ok(bytes.into_owned())
}

/// Call on a worker: the destination is created exclusively, never overwritten.
pub fn save_download(filename: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = dirs::download_dir().ok_or(Error::Invalid("Downloads folder unavailable"))?;
    std::fs::create_dir_all(&dir)?;
    save_in(&dir, filename, bytes)
}

fn save_in(dir: &Path, filename: &str, bytes: &[u8]) -> Result<PathBuf> {
    // Mail filenames are untrusted, including separators from other platforms.
    let name: String = filename.rsplit(['/', '\\']).next().unwrap_or_default().chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim_matches(['.', ' ']);
    let name = if name.is_empty() { "attachment" } else { name };
    let file = Path::new(name);
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("attachment");
    let extension = file.extension().and_then(|s| s.to_str()).map(|s| format!(".{s}")).unwrap_or_default();
    for index in 0..10_000 {
        let path = dir.join(if index == 0 { name.to_owned() } else { format!("{stem} ({index}){extension}") });
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error.into());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::Invalid("too many files with this attachment name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_decoding_preserves_binary_and_rejects_corruption() {
        assert_eq!(decode(b"AP9o\r\naQ==", "BASE64").unwrap(), b"\0\xffhi");
        assert_eq!(decode(b"=00=FFhi=\r\nthere", "quoted-printable").unwrap(), b"\0\xffhithere");
        for encoding in ["7bit", "8bit", "binary"] {
            assert_eq!(decode(b"\0\xff=", encoding).unwrap(), b"\0\xff=");
        }
        for (bytes, encoding) in [(b"!!!!".as_slice(), "base64"), (b"=ZZ", "quoted-printable"), (b"x", "unknown")] {
            assert!(decode(bytes, encoding).is_err());
        }
    }

    #[test]
    fn filenames_stay_in_destination_and_never_replace_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_in(dir.path(), "../../report.pdf", b"first").unwrap();
        let second = save_in(dir.path(), "C:\\folder\\report.pdf", b"second").unwrap();
        assert_eq!(first, dir.path().join("report.pdf"));
        assert_eq!(second, dir.path().join("report (1).pdf"));
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
        assert_eq!(save_in(dir.path(), "..", b"").unwrap(), dir.path().join("attachment"));
        assert_eq!(save_in(dir.path(), "bad\0:name", b"").unwrap(), dir.path().join("bad__name"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("report.pdf", dir.path().join("link.pdf")).unwrap();
            assert_eq!(save_in(dir.path(), "link.pdf", b"new").unwrap(), dir.path().join("link (1).pdf"));
            assert_eq!(std::fs::read(dir.path().join("report.pdf")).unwrap(), b"first");
        }
    }
}
