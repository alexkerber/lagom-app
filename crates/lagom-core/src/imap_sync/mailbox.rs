use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};

// IMAP4rev1 uses modified UTF-7 until UTF8=ACCEPT is explicitly enabled.
pub(super) fn encode(name: &str) -> String {
    let mut out = String::new();
    let mut wide = Vec::new();
    let flush = |wide: &mut Vec<u8>, out: &mut String| {
        if !wide.is_empty() {
            out.push('&');
            out.push_str(&STANDARD_NO_PAD.encode(&*wide).replace('/', ","));
            out.push('-');
            wide.clear();
        }
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut wide, &mut out);
            out.push(c);
            if c == '&' { out.push('-'); }
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) { wide.extend(unit.to_be_bytes()); }
        }
    }
    flush(&mut wide, &mut out);
    out
}

pub(crate) fn decode(name: &str) -> Result<String> {
    let invalid = || Error::Invalid("invalid IMAP mailbox encoding");
    let mut out = String::new();
    let mut rest = name;
    while let Some((plain, tail)) = rest.split_once('&') {
        out.push_str(plain);
        let (encoded, tail) = tail.split_once('-').ok_or_else(invalid)?;
        if encoded.is_empty() {
            out.push('&');
        } else {
            let bytes = STANDARD_NO_PAD.decode(encoded.replace(',', "/")).map_err(|_| invalid())?;
            if bytes.len() % 2 != 0 { return Err(invalid()); }
            let units: Vec<_> = bytes.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
            out.push_str(&String::from_utf16(&units).map_err(|_| invalid())?);
        }
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mailbox_names_roundtrip() {
        for (name, wire) in [("INBOX", "INBOX"), ("A&B", "A&-B"), ("台北/日本語", "&U,BTFw-/&ZeVnLIqe-")] {
            assert_eq!(encode(name), wire);
            assert_eq!(decode(wire).unwrap(), name);
        }
        for name in ["Räksmörgås", "Projects/📬", "two words", "\"quotes\""] {
            assert_eq!(decode(&encode(name)).unwrap(), name);
        }
        for name in ["&unterminated", "&!bad-", "&AA-", "&2AA-"] { assert!(decode(name).is_err()); }
    }
}
