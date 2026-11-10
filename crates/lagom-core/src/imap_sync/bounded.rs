use std::{io, pin::Pin, task::{Context, Poll, ready}};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(super) const MAX_LITERAL: usize = 64 * 1024 * 1024;
// One maximal response per command: every reply at the existing limits fits, an endless stream does not.
const COMMAND_BUDGET: usize = MAX_LITERAL + 65536;

// Distinguish our content limits from transport errors, including TLS InvalidData errors.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(super) struct Rejected(&'static str);

pub(super) fn rejected(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Rejected(reason))
}

#[derive(Debug)]
pub(super) struct Bounded<T> {
    pub(super) inner: T,
    pub remaining: Option<usize>,
    /// What the current command may still read, unsolicited responses included; reset by each write.
    command: usize,
    pending: Vec<u8>,
    needed: usize,
}
impl<T> Bounded<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, remaining: None, command: COMMAND_BUDGET, pending: Vec::new(), needed: 0 }
    }
    fn check(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(left) = &mut self.remaining { *left -= bytes.len(); }
        self.command -= bytes.len();
        self.pending.reserve_exact(bytes.len());
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= self.needed {
            // Use the same grammar: a separate literal scanner can disagree on unknown attributes.
            match async_imap::imap_proto::Response::parse(&self.pending) {
                Ok((rest, _)) => {
                    let consumed = self.pending.len() - rest.len();
                    self.pending.drain(..consumed);
                    self.needed = 0;
                    if self.pending.is_empty() {
                        self.pending = Vec::new();
                        break;
                    }
                }
                Err(nom::Err::Incomplete(needed)) => {
                    let extra = match needed { nom::Needed::Size(n) => n.get(), nom::Needed::Unknown => 1 };
                    self.needed = self.pending.len().checked_add(extra)
                        .ok_or_else(|| rejected("IMAP response too large"))?;
                    // async-imap reserves this much before its next read.
                    if self.needed > MAX_LITERAL + 65536 || extra > self.remaining.unwrap_or(usize::MAX).min(self.command) {
                        return Err(rejected("IMAP download limit exceeded"));
                    }
                    self.pending.reserve_exact(self.needed - self.pending.len());
                    break;
                }
                Err(_) => return Err(rejected("invalid IMAP response")),
            }
        }
        Ok(())
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for Bounded<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 { return Poll::Ready(Ok(())); }
        let max = buf.remaining().min(8192).min(this.remaining.unwrap_or(usize::MAX)).min(this.command)
            .min((MAX_LITERAL + 65536).saturating_sub(this.pending.len()));
        if max == 0 {
            this.remaining = Some(0);
            return Poll::Ready(Err(rejected("IMAP download limit exceeded")));
        }
        let mut read = ReadBuf::new(buf.initialize_unfilled_to(max));
        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut read))?;
        if let Err(error) = this.check(read.filled()) {
            this.remaining = Some(0);
            return Poll::Ready(Err(error));
        }
        let n = read.filled().len();
        buf.advance(n);
        Poll::Ready(Ok(()))
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Bounded<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.command = COMMAND_BUDGET;
        Pin::new(&mut this.inner).poll_write(cx, bytes)
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn fragmented_headers_reject_excess_before_reserving_the_body() {
        for wire in [
            b"* 1 FETCH (UID 1 BODY[1] {268435456}\r\n".as_slice(),
            b"* 1 FETCH (UID 1 X ({60}\r\n) BODY[1] {268435456}\r\n",
        ] {
            let (read, mut write) = tokio::io::duplex(1);
            let server = tokio::spawn(async move { let _ = write.write_all(wire).await; });
            let mut read = Bounded::new(read);
            let mut received = Vec::new();
            assert!(read.read_to_end(&mut received).await.unwrap_err().get_ref().is_some_and(|e| e.is::<Rejected>()));
            assert!(read.pending.capacity() < 8192);
            drop(read);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn binary_literals_and_multiple_responses_obey_the_total_budget() {
        let payload = b"\0\xff{268435456}\r\n";
        let mut wire = format!("* 1 FETCH (UID 1 BODY[1] {{{}}}\r\n", payload.len()).into_bytes();
        wire.extend_from_slice(payload);
        wire.extend_from_slice(b")\r\nA1 OK done\r\n");
        let (read, mut write) = tokio::io::duplex(1);
        let expected = wire.clone();
        let server = tokio::spawn(async move { write.write_all(&wire).await.unwrap(); });
        let mut read = Bounded::new(read);
        read.remaining = Some(expected.len() + 1);
        let mut received = Vec::new();
        read.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, expected);
        assert!(read.pending.is_empty());
        server.await.unwrap();

        let wire = b"* 1 FETCH (UID 1 BODY[1] {4}\r\ndata)\r\n".repeat(100);
        let mut read = Bounded::new(wire.as_slice());
        read.remaining = Some(128);
        received.clear();
        assert!(read.read_to_end(&mut received).await.unwrap_err().get_ref().is_some_and(|e| e.is::<Rejected>()));
        assert!(received.len() <= 128);
        assert!(read.pending.capacity() < 8192);
    }

    #[tokio::test]
    async fn each_command_reads_at_most_one_budget() {
        let wire = b"* 1 FETCH (UID 1 FLAGS ())\r\n".repeat(16);
        let mut read = Bounded::new(wire.as_slice());
        read.command = wire.len() / 2;
        let mut received = Vec::new();
        assert!(read.read_to_end(&mut received).await.unwrap_err().get_ref().is_some_and(|e| e.is::<Rejected>()));
        assert!(received.len() <= wire.len() / 2);
        let (wire, mut server) = tokio::io::duplex(64);
        let mut read = Bounded::new(wire);
        for _ in 0..3 {
            read.command = 64;
            read.write_all(b"A1 NOOP\r\n").await.unwrap();
            assert_eq!(read.command, COMMAND_BUDGET);
            server.write_all(b"A1 OK done\r\n").await.unwrap();
            let mut line = [0; 12];
            read.read_exact(&mut line).await.unwrap();
        }
        // A dead stream stays dead: the next command's budget must not revive it.
        read.remaining = Some(0);
        read.write_all(b"A2 NOOP\r\n").await.unwrap();
        server.write_all(b"A2 OK done\r\n").await.unwrap();
        assert!(read.read_exact(&mut [0; 12]).await.is_err());
    }
}
