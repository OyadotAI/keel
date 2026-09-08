//! Reading a child process's output without ever spinning on it.
//!
//! Three loops in the daemon drain a pipe line by line: the agent's stdout, the agent's stderr,
//! and a monitored background command. All three had the same shape —
//!
//! ```ignore
//! match lines.next_line().await {
//!     Ok(Some(line)) => …,
//!     Ok(None) => break,
//!     Err(_) => continue,
//! }
//! ```
//!
//! — and the same comment explaining it: an `Err` is a non-UTF-8 byte, and treating that like EOF
//! would stop the drain, leave the pipe to fill, and block the child mid-write. That reasoning is
//! right, and `continue` is right *for that error*, because tokio has already consumed the bytes
//! it could not decode and the next call carries on past them.
//!
//! It is wrong for every other error. A read that fails for a reason that does not go away — the
//! descriptor gone, `EIO` from a pty whose other end died — returns `Err` again immediately, and
//! `continue` turns that into a loop with no await point that ever blocks: one core at 100%, for
//! the life of the daemon, in a `tokio::spawn` nobody is watching. It has not been seen; it is
//! also two characters away at each of three sites, which is exactly the kind of thing that is
//! cheaper to make impossible than to remember.
//!
//! So the classification lives here, once, and the call sites say what they mean.

use tokio::io::{AsyncBufRead, Lines};

pub enum Next {
    /// A line the child wrote.
    Line(String),
    /// Bytes that were not text. Consumed and dropped; keep reading.
    Skipped,
    /// End of the pipe, or a read that will not recover. Stop.
    Done,
}

/// The next line, with "undecodable" and "broken" told apart.
pub async fn next<R: AsyncBufRead + Unpin>(lines: &mut Lines<R>) -> Next {
    match lines.next_line().await {
        Ok(Some(line)) => Next::Line(line),
        Ok(None) => Next::Done,
        // The one recoverable case: tokio consumed the bytes, could not make a `String` of them,
        // and the reader has moved past. Anything else is not going to be different next time.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Next::Skipped,
        Err(_) => Next::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    /// A non-UTF-8 line is dropped and the ones after it still arrive.
    #[tokio::test]
    async fn undecodable_bytes_do_not_end_the_drain() {
        let raw: Vec<u8> = b"first\n\xff\xfe\nthird\n".to_vec();
        let mut lines = tokio::io::BufReader::new(&raw[..]).lines();

        let mut got = Vec::new();
        let mut skipped = 0;
        loop {
            match next(&mut lines).await {
                Next::Line(l) => got.push(l),
                Next::Skipped => skipped += 1,
                Next::Done => break,
            }
        }
        assert_eq!(got, ["first", "third"]);
        assert_eq!(skipped, 1, "the undecodable line was skipped, not fatal");
    }

    /// And a reader that fails for good stops, rather than being asked again forever.
    #[tokio::test]
    async fn a_broken_reader_ends_the_drain() {
        struct Broken;
        impl tokio::io::AsyncRead for Broken {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("the pipe is gone")))
            }
        }
        let mut lines = tokio::io::BufReader::new(Broken).lines();
        assert!(
            matches!(next(&mut lines).await, Next::Done),
            "a permanent read error must end the loop, not spin it"
        );
    }
}
