use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<zeroize::Zeroizing<String>>,
    pub cmd: String,
    pub args: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    Unauthorized,
    Forbidden,
    NotFound,
    InvalidRequest,
    Denied,
    Conflict,
    Uncertain,
    Busy,
    Unavailable,
    Unsupported,
    TooLarge,
    Internal,
    AppUnreachable,
}

impl Code {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthorized => "Token missing, invalid, expired or revoked.",
            Self::Forbidden => "This agent's access level does not allow this command.",
            Self::NotFound => "Not found.",
            Self::InvalidRequest => "Invalid request.",
            Self::Denied => "Not approved.",
            Self::Conflict => "Changed or ambiguous. Read it again.",
            Self::Uncertain => "The outcome is unknown. Check before you retry.",
            Self::Busy => "Too many requests. Try again.",
            Self::Unavailable => "Temporarily unavailable. Try again.",
            Self::Unsupported => "Not supported.",
            Self::TooLarge => "Too large.",
            Self::Internal => "Internal error.",
            Self::AppUnreachable => "lagom is not running, or agent access is off.",
        }
    }
}

impl From<rusqlite::Error> for Code {
    fn from(err: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode::{DatabaseBusy, DatabaseLocked};
        match err.sqlite_error_code() {
            Some(DatabaseBusy | DatabaseLocked) => Self::Unavailable,
            _ => Self::Internal,
        }
    }
}

/// Each reply has exactly one payload. Unknown optional v1 fields are ignored.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "WireResponse")]
pub enum Response {
    Success(Value),
    Failure(Code),
}

#[derive(Deserialize)]
struct WireResponse {
    v: u32,
    ok: bool,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct ErrorMessage<'a> {
    code: Code,
    message: &'a str,
}

impl TryFrom<WireResponse> for Response {
    type Error = &'static str;

    fn try_from(mut wire: WireResponse) -> Result<Self, Self::Error> {
        if wire.v != VERSION {
            return Err(Code::Unsupported.message());
        }
        match (
            wire.ok,
            wire.fields.remove("result"),
            wire.fields.remove("error"),
        ) {
            (true, Some(result), None) => Ok(Self::Success(result)),
            (false, None, Some(value)) => {
                let error = ErrorMessage::deserialize(&value)
                    .map_err(|_| Code::InvalidRequest.message())?;
                if error.message != error.code.message() {
                    return Err(Code::InvalidRequest.message());
                }
                Ok(Self::Failure(error.code))
            }
            _ => Err(Code::InvalidRequest.message()),
        }
    }
}

impl Serialize for Response {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut reply = serializer.serialize_struct("Response", 3)?;
        reply.serialize_field("v", &VERSION)?;
        reply.serialize_field("ok", &matches!(self, Self::Success(_)))?;
        match self {
            Self::Success(result) => reply.serialize_field("result", result)?,
            Self::Failure(code) => reply.serialize_field(
                "error",
                &ErrorMessage {
                    code: *code,
                    message: code.message(),
                },
            )?,
        }
        reply.end()
    }
}
