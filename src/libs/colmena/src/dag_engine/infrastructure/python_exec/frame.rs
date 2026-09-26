//! Length-prefixed frames: a big-endian u32 length, then the bytes.

use std::io::{self, Read, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const FRAME_TOO_LARGE: &str = "frame exceeds the configured limit";

fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, FRAME_TOO_LARGE)
}

fn len_prefix(bytes: &[u8]) -> io::Result<[u8; 4]> {
    u32::try_from(bytes.len())
        .map(u32::to_be_bytes)
        .map_err(|_| too_large())
}

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(&len_prefix(bytes)?)?;
    w.write_all(bytes)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(too_large());
    }
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
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(too_large());
    }
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
        assert_eq!(err.to_string(), FRAME_TOO_LARGE);
    }

    #[tokio::test]
    async fn async_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(64);
        write_frame_async(&mut a, b"abc").await.unwrap();
        assert_eq!(read_frame_async(&mut b, 16).await.unwrap(), b"abc");
    }
}
