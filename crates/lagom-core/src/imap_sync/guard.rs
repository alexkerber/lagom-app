use std::{io, pin::Pin, task::{Context, Poll}};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

// Reject excessive wire nesting early; the patched parser also bounds recursion independently.
// shortcut: cap all structured responses at 40 lists; revisit if a supported extension needs deeper lists.
const MAX_DEPTH: usize = 40;

#[derive(Debug)]
pub(super) struct Guarded<T> {
    pub(super) inner: T,
    scanner: Scanner,
}

impl<T> Guarded<T> {
    pub(super) fn new(inner: T) -> Self {
        Self { inner, scanner: Scanner { state: State::Start, depth: 0 } }
    }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Start,
    Star,
    Kind([u8; 7], usize),
    Text,
    Syntax,
    Quoted,
    Escape,
    Size(u32, bool),
    LiteralCr(u32),
    LiteralLf(u32),
    Literal(u32),
    Failed,
}

#[derive(Debug)]
struct Scanner {
    state: State,
    depth: usize,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "IMAP response nesting or framing limit exceeded")
}

impl Scanner {
    fn check(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        if matches!(self.state, State::Failed) {
            return Err(invalid());
        }
        while let Some((&byte, rest)) = bytes.split_first() {
            if let State::Literal(remaining) = self.state {
                let skip = bytes.len().min(remaining as usize);
                bytes = &bytes[skip..];
                self.state = if skip == remaining as usize { State::Syntax } else { State::Literal(remaining - skip as u32) };
                continue;
            }
            bytes = rest;
            self.state = match self.state {
                State::Start if byte == b'*' => State::Star,
                State::Start | State::Text => if byte == b'\n' { State::Start } else { State::Text },
                State::Star if byte == b' ' => State::Kind([0; 7], 0),
                State::Star => State::Failed,
                State::Kind(word, len) if byte == b' ' => {
                    // Status and continuation text can contain arbitrary quotes, braces and parentheses.
                    if matches!(&word[..len], b"OK" | b"NO" | b"BAD" | b"BYE" | b"PREAUTH") { State::Text } else { State::Syntax }
                }
                State::Kind(_, _) if byte == b'\n' => State::Start,
                State::Kind(mut word, len) => {
                    if len == word.len() {
                        State::Syntax
                    } else {
                        word[len] = byte.to_ascii_uppercase();
                        State::Kind(word, len + 1)
                    }
                }
                State::Syntax => match byte {
                    b'(' if self.depth < MAX_DEPTH => { self.depth += 1; State::Syntax }
                    b')' if self.depth > 0 => { self.depth -= 1; State::Syntax }
                    b'(' | b')' => State::Failed,
                    b'"' => State::Quoted,
                    b'{' => State::Size(0, false),
                    b'\n' if self.depth == 0 => State::Start,
                    b'\n' => State::Failed,
                    _ => State::Syntax,
                },
                State::Quoted => match byte {
                    b'"' => State::Syntax,
                    b'\\' => State::Escape,
                    b'\r' | b'\n' => State::Failed,
                    _ => State::Quoted,
                },
                State::Escape => match byte {
                    b'"' | b'\\' => State::Quoted,
                    _ => State::Failed,
                },
                State::Size(size, _) if byte.is_ascii_digit() => {
                    match size.checked_mul(10).and_then(|n| n.checked_add(u32::from(byte - b'0'))) {
                        Some(n) => State::Size(n, true),
                        None => State::Failed,
                    }
                }
                State::Size(size, true) if byte == b'}' => State::LiteralCr(size),
                State::LiteralCr(size) if byte == b'\r' => State::LiteralLf(size),
                State::LiteralLf(size) if byte == b'\n' => if size == 0 { State::Syntax } else { State::Literal(size) },
                _ => State::Failed,
            };
            if matches!(self.state, State::Failed) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Guarded<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if matches!(this.scanner.state, State::Failed) {
            return Poll::Ready(Err(invalid()));
        }
        let start = buf.filled().len();
        std::task::ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        if let Err(error) = this.scanner.check(&buf.filled()[start..]) {
            // Never expose the offending read to async-imap, even if it contains complete responses.
            buf.set_filled(start);
            return Poll::Ready(Err(error));
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Guarded<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_imap::{imap_proto::Response, Client};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn nested(depth: usize) -> String {
        format!("* 1 FETCH (BODYSTRUCTURE {}(\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1){})\r\n",
            "(".repeat(depth), " \"MIXED\")".repeat(depth))
    }

    #[test]
    fn depth_limit_survives_chunk_boundaries() {
        for depth in [MAX_DEPTH, MAX_DEPTH + 1, 10_000] {
            let raw = format!("* 1 FETCH {}{}\r\n", "(".repeat(depth), ")".repeat(depth));
            for chunk in [1, 2, 7, raw.len()] {
                let mut scanner = Scanner { state: State::Start, depth: 0 };
                let result = raw.as_bytes().chunks(chunk).try_for_each(|bytes| scanner.check(bytes));
                assert_eq!(result.is_ok(), depth == MAX_DEPTH);
                if result.is_err() {
                    assert!(scanner.check(b"* OK recovered\r\n").is_err());
                }
            }
        }
    }

    #[tokio::test]
    async fn text_quotes_and_literals_pass_unchanged() {
        let payload = format!("{}\r\n\"\\{{123}}\r\n{}", "(".repeat(1000), nested(100));
        let raw = format!(
            "* OK greeting \"{{999}}\r\n+ challenge (\r\na1 OK done )\r\n* LIST () \"/\" {{{}}}\r\n{payload}\r\n* 1 FETCH (BODY[TEXT] {{0}}\r\n)\r\n* LIST () \"/\" \"\\\"\\\\{}\"\r\n",
            payload.len(), "(".repeat(1000),
        );
        for chunk in [1, 3, 4096] {
            let (mut server, client) = tokio::io::duplex(chunk);
            let expected = raw.clone();
            let writer = tokio::spawn(async move { server.write_all(expected.as_bytes()).await.unwrap(); });
            let mut actual = Vec::new();
            Guarded::new(client).read_to_end(&mut actual).await.unwrap();
            assert_eq!(actual, raw.as_bytes());
            writer.await.unwrap();
        }
    }

    #[test]
    fn malformed_framing_fails_closed() {
        for suffix in ["{4294967296}\r\n", "{}\r\n", "{1}\n", "{1+}\r\n", "\"bad\\x\"", "\"bad\r\n", "(\r\n", ")"] {
            let raw = format!("* 1 FETCH {suffix}");
            let mut scanner = Scanner { state: State::Start, depth: 0 };
            assert!(raw.as_bytes().chunks(1).try_for_each(|b| scanner.check(b)).is_err(), "{suffix:?}");
            assert!(scanner.check(b"").is_err());
        }
    }

    #[tokio::test]
    async fn parser_receives_only_bounded_bodystructures() {
        for depth in [32, MAX_DEPTH - 2, MAX_DEPTH - 1, 10_000] {
            for chunk in [1, 4096] {
                let message = format!("* 1 FETCH (BODYSTRUCTURE {}(\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1){})\r\n",
                    "(\"MESSAGE\" \"RFC822\" NIL NIL NIL \"7BIT\" 4 (NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL) ".repeat(depth), " 1)".repeat(depth));
                let extension = format!("* 1 FETCH (BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1 NIL NIL NIL NIL {}NIL{}))\r\n",
                    "(".repeat(depth), ")".repeat(depth));
                for raw in [nested(depth), message, extension] {
                    let (mut server, stream) = tokio::io::duplex(chunk);
                    let writer = tokio::spawn(async move { let _ = server.write_all(raw.as_bytes()).await; });
                    let mut client = Client::new(Guarded::new(stream));
                    let response = client.read_response().await;
                    if depth <= MAX_DEPTH - 2 {
                        assert!(matches!(response.unwrap().unwrap().parsed(), Response::Fetch(..)));
                    } else {
                        let error = response.unwrap_err();
                        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                        assert_eq!(error.to_string(), invalid().to_string());
                        assert!(client.read_response().await.unwrap().is_none());
                    }
                    drop(client);
                    writer.await.unwrap();
                }
            }
        }
    }

    #[tokio::test]
    async fn unknown_attribute_cannot_hide_fragmented_recursion() {
        for depth in [48, 10_000] {
            for chunk in [1, 7, 4096] {
                let raw = format!("* 1 FETCH (X (\") BODYSTRUCTURE {}", "(".repeat(depth));
                let (mut server, stream) = tokio::io::duplex(chunk);
                let writer = tokio::spawn(async move {
                    let _ = server.write_all(raw.as_bytes()).await;
                    std::future::pending::<()>().await;
                });
                let mut client = Client::new(Guarded::new(stream));
                let response = tokio::time::timeout(std::time::Duration::from_secs(1), client.read_response()).await;
                writer.abort();
                let _ = writer.await;
                let error = response.expect("must reject recursion before the server finishes the response").unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert!(client.read_response().await.unwrap().is_none());
            }
        }
    }

    #[tokio::test]
    async fn literal_and_status_text_cannot_hide_a_later_attack() {
        let payload = nested(100);
        let raw = format!("* LIST () \"/\" {{{}}}\r\n{payload}\r\n* OK {{999999}}\r\n{}", payload.len(), nested(100));
        for chunk in [1, 4096] {
            let (mut server, stream) = tokio::io::duplex(chunk);
            let raw = raw.clone();
            let writer = tokio::spawn(async move { let _ = server.write_all(raw.as_bytes()).await; });
            let mut guarded = Guarded::new(stream);
            assert_eq!(guarded.read_to_end(&mut Vec::new()).await.unwrap_err().kind(), io::ErrorKind::InvalidData);
            drop(guarded);
            writer.await.unwrap();
        }
    }
}
