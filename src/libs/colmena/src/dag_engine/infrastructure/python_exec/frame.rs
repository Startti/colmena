//! Length-prefixed frames: a big-endian u32 length, then the bytes.

use std::io::{self, Read, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const FRAME_TOO_LARGE: &str = "frame exceeds the configured limit";

/// Marker carried inside the `io::Error` so callers can identify the
/// too-large case without matching on its message text.
#[derive(Debug)]
struct FrameTooLarge;

impl std::fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(FRAME_TOO_LARGE)
    }
}

impl std::error::Error for FrameTooLarge {}

fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, FrameTooLarge)
}

/// True when `e` is the error [`read_frame`]/[`read_frame_async`] return for a
/// length prefix over the caller's cap.
pub fn is_frame_too_large(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<FrameTooLarge>())
}

fn len_prefix(bytes: &[u8]) -> io::Result<[u8; 4]> {
    u32::try_from(bytes.len())
        .map(u32::to_be_bytes)
        .map_err(|_| too_large())
}

/// Decodes the big-endian length prefix and rejects it against `max` before
/// any body bytes are read or a buffer for them is allocated.
fn checked_len(prefix: [u8; 4], max: usize) -> io::Result<usize> {
    let n = u32::from_be_bytes(prefix) as usize;
    if n > max {
        return Err(too_large());
    }
    Ok(n)
}

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(&len_prefix(bytes)?)?;
    w.write_all(bytes)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = checked_len(len, max)?;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub async fn write_frame_async(w: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> io::Result<()> {
    w.write_all(&len_prefix(bytes)?).await?;
    w.write_all(bytes).await?;
    w.flush().await
}

pub async fn read_frame_async(r: &mut (impl AsyncRead + Unpin), max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let n = checked_len(len, max)?;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_cap() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        assert_eq!(read_frame(&mut &buf[..], 5).unwrap(), b"hello");
        let err = read_frame(&mut &buf[..], 4).unwrap_err();
        assert!(is_frame_too_large(&err));
        assert_eq!(err.to_string(), FRAME_TOO_LARGE);
    }

    #[tokio::test]
    async fn async_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(64);
        write_frame_async(&mut a, b"abc").await.unwrap();
        assert_eq!(read_frame_async(&mut b, 16).await.unwrap(), b"abc");
    }

    /// A prefix over the cap with no body at all must be rejected by the cap
    /// check itself, not discovered later as a truncated read: pins the cap
    /// being checked before the body is allocated or read.
    #[test]
    fn oversized_prefix_with_no_body_is_frame_too_large() {
        let prefix = [0xFF, 0xFF, 0xFF, 0xFF];
        let err = read_frame(&mut &prefix[..], 16).unwrap_err();
        assert!(is_frame_too_large(&err));
    }

    #[test]
    fn prefix_promising_more_than_the_body_holds_is_unexpected_eof() {
        let mut buf = 10u32.to_be_bytes().to_vec();
        buf.extend_from_slice(&[1, 2, 3]);
        let err = read_frame(&mut &buf[..], 16).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn async_oversized_prefix_is_frame_too_large_before_reading_body() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&[0xFF, 0xFF, 0xFF, 0xFF]).await.unwrap();
        drop(a);
        let err = read_frame_async(&mut b, 16).await.unwrap_err();
        assert!(is_frame_too_large(&err));
    }
}
