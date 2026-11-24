use crate::{
    store::{fts_query, summary_row, MessageSummary, Store, VISIBLE_SUMMARY_COLUMNS},
    Error, Result,
};
use chrono::NaiveDate;
use rusqlite::{params_from_iter, types::Value};

const INVALID: Error = Error::Invalid("invalid_filter");
const MAX_BYTES: usize = 4096;
const MAX_TERMS: usize = 64;

/// Adjacent terms are required. OR joins alternatives within one required group.
#[derive(Debug)]
pub struct Filter(Vec<Vec<Term>>);

#[derive(Debug)]
struct Term {
    negated: bool,
    predicate: Predicate,
}

#[derive(Debug)]
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
            let (operator, rest) = match suffix.strip_prefix(':') {
                Some(value) => (Some(head), value),
                None => (None, rest),
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
            if operator.is_none() && !quoted && value == "OR" {
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
                None if quoted => Predicate::FullText(format!("\"{value}\"")),
                None => Predicate::FullText(fts_query(value).ok_or(INVALID)?),
                Some("from") => Predicate::From(value.into()),
                Some("to") => Predicate::To(value.into()),
                Some("cc") => Predicate::Cc(value.into()),
                Some("subject") => Predicate::Subject(value.into()),
                Some("is") if value == "unread" || value == "read" => {
                    Predicate::Seen(value == "read")
                }
                Some("is") if value == "starred" => Predicate::Starred,
                Some("has") if value == "attachment" => Predicate::Attachment,
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
            groups.last_mut().unwrap().push(Term { negated, predicate });
            alternative = false;
        }
        if alternative {
            return Err(INVALID);
        }
        Ok(Self(groups))
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
    fn sql(&self, now: i64, args: &mut Vec<Value>) -> String {
        let mut bind = |value| {
            args.push(value);
            format!("?{}", args.len())
        };
        match self {
            Self::From(s) => {
                let p = bind(s.clone().into());
                format!("(instr(lower(m.from_address),lower({p})) OR instr(lower(m.from_name),lower({p})))")
            }
            Self::To(s) => format!(
                "instr(lower(m.to_addresses),lower({}))",
                bind(s.clone().into())
            ),
            Self::Cc(s) => format!("instr(lower(m.cc),lower({}))", bind(s.clone().into())),
            Self::Subject(s) => {
                format!("instr(lower(m.subject),lower({}))", bind(s.clone().into()))
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
                "inbox" | "archive" | "sent" => format!(
                    "(f.role={} AND (m.snoozed_until IS NULL OR m.snoozed_until<={}))",
                    bind(s.to_ascii_lowercase().into()),
                    bind(now.into())
                ),
                _ => format!("f.name={} COLLATE NOCASE", bind(s.clone().into())),
            },
            Self::After(t) => format!("m.date>={}", bind((*t).into())),
            Self::Before(t) => format!("m.date<{}", bind((*t).into())),
            Self::Newer(age) => format!("m.date>{}", bind(now.saturating_sub(*age).into())),
            Self::Older(age) => format!("m.date<{}", bind(now.saturating_sub(*age).into())),
        }
    }
}

impl Store {
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
            return Err(Error::Invalid("invalid_request"));
        }
        if scope.is_some_and(|s| {
            s.trim().is_empty() || s.len() > MAX_BYTES || s.chars().any(char::is_control)
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
            sql.push_str(&Predicate::Folder(scope.into()).sql(now, &mut args));
        }
        for group in &filter.0 {
            sql.push_str(" AND (");
            for (i, term) in group.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                if term.negated {
                    sql.push_str("NOT ");
                }
                sql.push_str(&term.predicate.sql(now, &mut args));
            }
            sql.push(')');
        }
        sql.push_str(" ORDER BY m.date DESC,m.id DESC LIMIT ? OFFSET ?");
        args.push((limit + 1).into());
        args.push((i64::from(page - 1) * i64::from(limit)).into());
        let mut stmt = self.db.prepare(&sql)?;
        let mut rows = stmt
            .query_map(params_from_iter(args), summary_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = if rows.len() > limit as usize {
            rows.pop();
            page.checked_add(1)
        } else {
            None
        };
        Ok((rows, next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "private:secret",
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
                "invalid_filter"
            );
        }
        assert!(Filter::parse(&"a".repeat(MAX_BYTES)).is_ok());
        assert!(Filter::parse(&"a".repeat(MAX_BYTES + 1)).is_err());
        assert!(Filter::parse(&"x ".repeat(MAX_TERMS)).is_ok());
        assert!(Filter::parse(&"x ".repeat(MAX_TERMS + 1)).is_err());
    }
}
