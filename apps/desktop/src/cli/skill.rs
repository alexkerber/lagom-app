use super::*;

pub(super) fn install(words: &[String], args: &Args) -> Result<()> {
    if words != ["install"] || args.keys().any(|k| k != "dir" && k != "for") {
        return Err(INVALID);
    }
    let kind = args.get("for").and_then(Value::as_str).unwrap_or("codex");
    let config = match kind {
        "claude" => ".claude",
        "codex" => ".codex",
        "hermes" => ".hermes",
        "openclaw" => ".openclaw",
        "other" if args.contains_key("dir") => "",
        _ => return Err(INVALID),
    };
    let dir = match args.get("dir").and_then(Value::as_str) {
        Some(path) => PathBuf::from(path),
        None => dirs::home_dir().ok_or(Code::Unavailable)?.join(config).join("skills/lagom"),
    };
    let write = || -> io::Result<()> {
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_symlink()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join("SKILL.md");
        if std::fs::symlink_metadata(&dest).is_ok_and(|m| !m.is_file()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.write_all(include_bytes!("../../assets/skill/SKILL.md"))?;
        file.persist(dest)?;
        Ok(())
    };
    write().map_err(|_| Code::Unavailable)
}
