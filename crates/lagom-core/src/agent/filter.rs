use crate::{
    store::{Store,summary_row, MessageSummary, VISIBLE_SUMMARY_COLUMNS},
    Error, Result,
};
use chrono::NaiveDate;
use rusqlite::{
    functions::FunctionFlags, params_from_iter, types::Value, Connection, ErrorCode, OpenFlags,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use unicase::UniCase;

const INVALID: Error = Error::Invalid("invalid_request");
const MAX_BYTES: usize = 4096;
const MAX_OFFSET: i64 = 10_000;
const MAX_TERMS: usize = 64;

/// Adjacent terms are required. OR joins alternatives within one required group.
#[derive(Debug)]
pub struct Filter(Vec<Vec<Term>>);

#[derive(Debug, PartialEq)]
struct Term {
    negated: bool,
    predicate: Predicate,
}

#[derive(Debug, PartialEq)]
enum Predicate {
    From(String),
    To(String),
    Cc(String),
    Subject(String),
    FullText(String),
    Seen(bool),
    Starred,
    Attachment,
    Folder(String),
    After(i64),
    Before(i64),
    Newer(i64),
    Older(i64),
}

impl Filter {
    /// Dates start at midnight UTC. Relative months and years are 30 and 365 days.
    pub fn parse(mut input: &str) -> Result<Self> {
        if input.len() > MAX_BYTES || input.chars().any(|c| c.is_control() && !c.is_whitespace()) {
            return Err(INVALID);
        }
        let mut groups: Vec<Vec<Term>> = Vec::new();
        let mut terms = 0;
        let mut alternative = false;
        loop {
            input = input.trim_start();
            if input.is_empty() {
                break;
            }
            let (negated, rest) = match input.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, input),
            };
            let end = rest
                .find(|c: char| c == ':' || c == '"' || c.is_whitespace())
                .unwrap_or(rest.len());
            let (head, suffix) = rest.split_at(end);
            let head = head.to_ascii_lowercase();
            let (operator, rest) = match (head.as_str(), suffix.strip_prefix(':')) {
                (
                    op @ ("from" | "to" | "cc" | "subject" | "is" | "has" | "in" | "after"
                    | "before" | "newer_than" | "older_than"),
                    Some(value),
                ) => (Some(op), value),
                _ => (None, rest),
            };
            let quoted = rest.starts_with('"');
            let (value, tail) = if let Some(rest) = rest.strip_prefix('"') {
                rest.split_once('"').ok_or(INVALID)?
            } else {
                rest.split_at(rest.find(char::is_whitespace).unwrap_or(rest.len()))
            };
            if value.trim().is_empty()
                || value.contains('"')
                || !tail.is_empty() && !tail.starts_with(char::is_whitespace)
            {
                return Err(INVALID);
            }
            input = tail;
            if operator.is_none() && !quoted && value.eq_ignore_ascii_case("OR") {
                if negated || terms == 0 || alternative {
                    return Err(INVALID);
                }
                alternative = true;
                continue;
            }
            terms += 1;
            if terms > MAX_TERMS {
                return Err(INVALID);
            }
            let predicate = match operator {
                None => {
                    // FTS prefixes apply to the last token in a quoted string.
                    let last_word = value.rsplit(|c: char| !c.is_alphanumeric()).next().unwrap();
                    let prefix = if !quoted && last_word.chars().count() >= 3 {
                        "*"
                    } else {
                        ""
                    };
                    Predicate::FullText(format!("\"{value}\"{prefix}"))
                }
                Some("from") => Predicate::From(UniCase::new(value).to_folded_case()),
                Some("to") => Predicate::To(UniCase::new(value).to_folded_case()),
                Some("cc") => Predicate::Cc(UniCase::new(value).to_folded_case()),
                Some("subject") => Predicate::Subject(UniCase::new(value).to_folded_case()),
                Some("is") if value.eq_ignore_ascii_case("unread") => Predicate::Seen(false),
                Some("is") if value.eq_ignore_ascii_case("read") => Predicate::Seen(true),
                Some("is") if value.eq_ignore_ascii_case("starred") => Predicate::Starred,
                Some("has") if value.eq_ignore_ascii_case("attachment") => Predicate::Attachment,
                Some("in") => Predicate::Folder(value.into()),
                Some("after") => Predicate::After(midnight(value)?),
                Some("before") => Predicate::Before(midnight(value)?),
                Some("newer_than") => Predicate::Newer(age_seconds(value)?),
                Some("older_than") => Predicate::Older(age_seconds(value)?),
                _ => return Err(INVALID),
            };
            if !alternative {
                groups.push(Vec::new());
            }
            let group = groups.last_mut().unwrap();
            let term = Term { negated, predicate };
            if !group.contains(&term) {
                group.push(term);
            }
            alternative = false;
        }
        if alternative {
            return Err(INVALID);
        }
        let mut unique = Vec::new();
        for group in groups {
            if !unique.contains(&group) {
                unique.push(group);
            }
        }
        Ok(Self(unique))
    }
}

fn midnight(value: &str) -> Result<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || !matches!(bytes[4], b'/' | b'-')
        || bytes[4] != bytes[7]
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(INVALID);
    }
    let date = NaiveDate::parse_from_str(
        value,
        if bytes[4] == b'/' {
            "%Y/%m/%d"
        } else {
            "%Y-%m-%d"
        },
    )
    .map_err(|_| INVALID)?;
    Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn age_seconds(value: &str) -> Result<i64> {
    let (unit, count) = value.as_bytes().split_last().ok_or(INVALID)?;
    let days = match unit {
        b'd' => 1,
        b'w' => 7,
        b'm' => 30,
        b'y' => 365,
        _ => return Err(INVALID),
    };
    if !count.iter().all(u8::is_ascii_digit) {
        return Err(INVALID);
    }
    value[..count.len()]
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(days * 86400))
        .ok_or(INVALID)
}

impl Predicate {
    fn sql(&self, now: i64, negated: bool, args: &mut Vec<Value>) -> String {
        let mut bind = |value| {
            args.push(value);
            format!("?{}", args.len())
        };
        match self {
            Self::From(s) => {
                let p = bind(s.clone().into());
                format!("(instr(casefold(m.from_address),{p}) OR instr(casefold(m.from_name),{p}))")
            }
            Self::To(s) => format!("instr(casefold(m.to_addresses),{})", bind(s.clone().into())),
            Self::Cc(s) => format!("instr(casefold(m.cc),{})", bind(s.clone().into())),
            Self::Subject(s) => {
                format!("instr(casefold(m.subject),{})", bind(s.clone().into()))
            }
            Self::FullText(q) => format!(
                "m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {})",
                bind(q.clone().into())
            ),
            Self::Seen(seen) => format!("m.seen={}", i32::from(*seen)),
            Self::Starred => "m.flagged".into(),
            Self::Attachment => "m.has_attachments".into(),
            Self::Folder(s) => match s.to_ascii_lowercase().as_str() {
                "all" => "1".into(),
                "later" => format!("coalesce(m.snoozed_until>{},0)", bind(now.into())),
                role => {
                    let folder = match role {
                        "inbox" | "archive" | "sent" | "trash" | "drafts" | "junk" => {
                            format!("f.role={}", bind(role.to_owned().into()))
                        }
                        "spam" => "f.role='junk'".into(),
                        _ => format!("f.name={} COLLATE NOCASE", bind(s.clone().into())),
                    };
                    if negated {
                        return folder;
                    }
                    format!(
                        "({folder} AND (m.snoozed_until IS NULL OR m.snoozed_until<={}))",
                        bind(now.into())
                    )
                }
            },
            Self::After(t) => format!("m.date>={}", bind((*t).into())),
            Self::Before(t) => format!("m.date<{}", bind((*t).into())),
            Self::Newer(age) => format!("m.date>{}", bind(now.saturating_sub(*age).into())),
            Self::Older(age) => format!("m.date<{}", bind(now.saturating_sub(*age).into())),
        }
    }
}

/// The dispatcher opens a dedicated query-only connection for agent searches.
pub struct FilterReader {
    store: Store,
}

impl FilterReader {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.pragma_update(None, "query_only", true)?;
        // A lock wait cannot be interrupted by the progress handler.
        db.busy_timeout(Duration::ZERO)?;
        register_casefold(&db)?;
        Ok(Self { store: Store { db } })
    }

    /// The largest permitted offset is 10,000 rows.
    /// Returns the page and its successor, if another matching row exists.
    pub fn filtered_messages(
        &mut self,
        accounts: &[String],
        filter: &Filter,
        scope: Option<&str>,
        now: i64,
        page: u32,
        limit: Option<u32>,
    ) -> Result<(Vec<MessageSummary>, Option<u32>)> {
        let deadline = Instant::now() + Duration::from_secs(2);
        self.store.db.progress_handler(100, Some(move || Instant::now() >= deadline))?;
        let result = self.store.filtered_messages(accounts, filter, scope, now, page, limit);
        self.store.db.progress_handler(0, None::<fn() -> bool>)?;
        result
    }
}

pub(super) fn register_casefold(db: &Connection) -> rusqlite::Result<()> {
        db.create_scalar_function(
            "casefold",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| Ok(UniCase::new(ctx.get_raw(0).as_str()?).to_folded_case()),
        )
}

impl Store {
    /// The largest permitted offset is 10,000 rows.
    /// Returns the page and its successor, if another matching row exists.
    pub fn filtered_messages(
        &self,
        accounts: &[String],
        filter: &Filter,
        scope: Option<&str>,
        now: i64,
        page: u32,
        limit: Option<u32>,
    ) -> Result<(Vec<MessageSummary>, Option<u32>)> {
        let limit = limit.unwrap_or(20).min(100);
        if page == 0 || limit == 0 {
            return Err(INVALID);
        }
        let offset = i64::from(page - 1) * i64::from(limit);
        if offset > MAX_OFFSET {
            return Err(INVALID);
        }
        if scope.is_some_and(|s| {
            s.len() > MAX_BYTES || s.trim().is_empty() || s.chars().any(char::is_control)
        }) {
            return Err(INVALID);
        }
        if accounts.is_empty() {
            return Ok((Vec::new(), None));
        }
        let mut args: Vec<Value> = accounts.iter().cloned().map(Value::Text).collect();
        let slots = vec!["?"; accounts.len()].join(",");
        let mut sql = format!(
            "SELECT {VISIBLE_SUMMARY_COLUMNS} FROM messages m
            JOIN folders f ON f.id=coalesce(m.move_target,m.folder) AND f.account=m.account
            WHERE m.account IN ({slots})"
        );
        if let Some(scope) = scope {
            sql.push_str(" AND ");
            sql.push_str(&Predicate::Folder(scope.into()).sql(now, false, &mut args));
        }
        let mut explicit_folders = Vec::new();
        for group in &filter.0 {
            sql.push_str(" AND (");
            for (i, term) in group.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                if term.negated {
                    sql.push_str("NOT ");
                }
                let clause = term.predicate.sql(now, term.negated, &mut args);
                sql.push_str(&clause);
                if !term.negated && matches!(term.predicate, Predicate::Folder(_)) {
                    explicit_folders.push(clause);
                }
            }
            sql.push(')');
        }
        if scope.is_none() {
            sql.push_str(" AND (f.role NOT IN ('trash','junk')");
            for folder in explicit_folders {
                sql.push_str(" OR ");
                sql.push_str(&folder);
            }
            sql.push(')');
        }
        sql.push_str(" ORDER BY m.date DESC,m.id DESC LIMIT ? OFFSET ?");
        args.push((limit + 1).into());
        args.push(offset.into());
        let mut rows = self.filter_rows(&sql,args)?;
        let next = if rows.len() > limit as usize {
            rows.pop();
            (offset + i64::from(limit) <= MAX_OFFSET).then_some(page + 1)
        } else {
            None
        };
        Ok((rows, next))
    }

    fn filter_rows(
        &self,
        sql: &str,
        args: Vec<Value>,
    ) -> Result<Vec<MessageSummary>> {
        let result = self.db.prepare(sql).and_then(|mut stmt| {
            stmt.query_map(params_from_iter(args), summary_row)?
                .collect()
        });
        match result {
            Err(e)
                if matches!(
                    e.sqlite_error_code(),
                    Some(
                        ErrorCode::OperationInterrupted
                            | ErrorCode::DatabaseBusy
                            | ErrorCode::DatabaseLocked
                    )
                ) =>
            {
                Err(Error::Invalid("unavailable"))
            }
            result => Ok(result?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_filter_deadline_interrupts_sqlite_and_releases_handler() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let reader = Store::agent_reader(file.path(), || true).unwrap();
        assert!(reader
            .db
            .pragma_query_value(None, "query_only", |r| r.get::<_, bool>(0))
            .unwrap());
        let sql = "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1000)
                   SELECT sum(x) FROM n HAVING sum(x)=0";
        assert!(matches!(
            reader.filter_rows(sql, vec![]),
            Err(Error::Invalid("unavailable"))
        ));
        drop(reader);
        let reader = Store::agent_reader(file.path(), || false).unwrap();
        assert!(reader
            .db
            .prepare(sql)
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none());
    }

    #[test]
    fn agent_filter_grammar_and_limits() {
        let filter = Filter::parse(r#"from:"Ada Lovelace" OR -cc:team "OR" rust"#).unwrap();
        assert_eq!(filter.0.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1]);
        assert!(filter.0[0][1].negated);
        assert!(matches!(&filter.0[1][0].predicate, Predicate::FullText(q) if q == "\"OR\""));
        assert_eq!(
            midnight("2024/02/29").unwrap(),
            midnight("2024-02-29").unwrap()
        );
        for (age, days) in [("2d", 2), ("2w", 14), ("2m", 60), ("2y", 730)] {
            assert_eq!(age_seconds(age).unwrap(), days * 86400);
        }
        for bad in [
            "subject:",
            "to:\"\"",
            "is:draft",
            "has:trash",
            "after:2023/02/29",
            "before:2024-2-01",
            "after:2024/12-01",
            "older_than:2h",
            "newer_than:-1w",
            "older_than:999999999999999999y",
            "OR x",
            "x OR",
            "x OR OR y",
            "-",
            "- from:x",
            "x\0y",
            "\"open",
            "\"two\"words",
            "x\"y\"",
            "from: \"name\"",
            "-OR",
        ] {
            assert_eq!(
                Filter::parse(bad).unwrap_err().to_string(),
                "invalid_request"
            );
        }
        assert!(Filter::parse(&"a".repeat(MAX_BYTES)).is_ok());
        assert!(Filter::parse(&"a".repeat(MAX_BYTES + 1)).is_err());
        assert_eq!(Filter::parse(&"x ".repeat(MAX_TERMS)).unwrap().0.len(), 1);
        assert_eq!(Filter::parse("x OR x").unwrap().0[0].len(), 1);
        assert!(Filter::parse(&"x ".repeat(MAX_TERMS + 1)).is_err());
    }
}
