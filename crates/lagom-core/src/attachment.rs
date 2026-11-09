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

/// Bidi and format controls let `invoice\u{202E}fdp.exe` display as `invoiceexe.pdf`.
pub(crate) fn strip_bidi(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}')).collect()
}

/// Call on a worker: the destination is created exclusively, never overwritten.
pub fn save_download(filename: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = dirs::download_dir().ok_or(Error::Invalid("Downloads folder unavailable"))?;
    std::fs::create_dir_all(&dir)?;
    save_in(&dir, filename, bytes)
}

fn save_in(dir: &Path, filename: &str, bytes: &[u8]) -> Result<PathBuf> {
    // Mail filenames are untrusted, including separators from other platforms.
    let name: String = strip_bidi(filename.rsplit(['/', '\\']).next().unwrap_or_default()).chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim_matches(['.', ' ']);
    let name = if name.is_empty() { "attachment" } else { name };
    let file = Path::new(name);
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("attachment");
    let extension = file.extension().and_then(|s| s.to_str()).map(|s| format!(".{s}")).unwrap_or_default();
    // UTF-8 bytes also bound UTF-16 units; reserve the longest collision suffix.
    // shortcut: extensions beyond 32 bytes are shortened; revisit if a file type needs more.
    let extension = truncate(&extension, 32).trim_end_matches(['.', ' ']);
    let stem = truncate(stem, 255 - "_ (9999)".len() - extension.len()).trim_end_matches(['.', ' ']);
    let base = stem.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base.strip_prefix("COM").or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
    let prefix = if device { "_" } else { "" };
    for index in 0..10_000 {
        let suffix = if index == 0 { String::new() } else { format!(" ({index})") };
        let path = dir.join(format!("{prefix}{stem}{suffix}{extension}"));
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

fn truncate(value: &str, max: usize) -> &str {
    let mut end = value.len().min(max);
    while !value.is_char_boundary(end) { end -= 1; }
    &value[..end]
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
    fn bidi_and_format_controls_are_stripped() {
        assert_eq!(strip_bidi("invoice\u{202E}fdp.exe"), "invoicefdp.exe");
        assert_eq!(strip_bidi("\u{FEFF}a\u{200E}\u{200F}\u{202A}\u{202B}\u{202C}\u{202D}b\u{2066}\u{2067}\u{2068}\u{2069}\u{061C}.pdf"), "ab.pdf");
        assert_eq!(strip_bidi("räksmörgås שלום.pdf"), "räksmörgås שלום.pdf");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(save_in(dir.path(), "invoice\u{202E}fdp.exe", b"").unwrap(), dir.path().join("invoicefdp.exe"));
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["CON", "prn.txt", "Aux.tar.gz", "nul", "cOm9.txt", "LPT1.pdf", "COM¹.txt", "lpt²", "COM³", "CON .txt"] {
            let path = save_in(dir.path(), name, b"file").unwrap();
            assert_eq!(path.file_name().unwrap().to_str().unwrap(), format!("_{}", name.replace(" .", ".")));
            assert_eq!(std::fs::read(path).unwrap(), b"file");
        }
        for name in ["COM0.txt", "COM10.txt", "LPT0", "console.txt"] {
            assert_eq!(save_in(dir.path(), name, b"").unwrap(), dir.path().join(name));
        }
    }

    #[test]
    fn long_filenames_leave_room_for_extensions_and_collisions() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a".repeat(300), format!("{}.txt", "b".repeat(251)), format!("{}.pdf", "界😀".repeat(80)), format!("report.{}", "é".repeat(200))] {
            let first = save_in(dir.path(), &name, b"first").unwrap();
            let second = save_in(dir.path(), &name, b"second").unwrap();
            assert_ne!(first, second);
            for path in [&first, &second] {
                let filename = path.file_name().unwrap().to_str().unwrap();
                assert!(filename.len() <= 255);
                assert!(!filename.ends_with(['.', ' ']));
            }
            if name.ends_with(".txt") || name.ends_with(".pdf") {
                assert_eq!(first.extension(), Path::new(&name).extension());
                assert_eq!(second.extension(), first.extension());
            }
            assert_eq!(std::fs::read(first).unwrap(), b"first");
            assert_eq!(std::fs::read(second).unwrap(), b"second");
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
