//! Raw zip archives for tests: every header field of the central directory and
//! of the local header can be set independently, so an archive can lie in
//! exactly one place. Entries are stored (never deflated); the `method` field is
//! only a claim, which is all a header-only check ever reads.

/// CRC-32 (IEEE), bitwise: small and good enough for fixtures.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[derive(Clone, Copy)]
pub(crate) struct Sizes {
    pub csize: u32,
    pub usize_: u32,
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    pub central: Sizes,
    pub local: Sizes,
    /// Where the central directory says the local header is, when it is not
    /// where it was written.
    pub offset_override: Option<u32>,
    /// What the local header says the name is (a lie), if different.
    pub local_name: Option<Vec<u8>>,
}

impl Entry {
    /// A well-formed stored entry.
    pub fn stored(name: &str, data: &[u8]) -> Self {
        let s = Sizes {
            csize: data.len() as u32,
            usize_: data.len() as u32,
        };
        Self {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            method: 0,
            flags: 0,
            central: s,
            local: s,
            offset_override: None,
            local_name: None,
        }
    }

    /// Both headers claim these sizes and this method (consistently).
    pub fn claim(mut self, method: u16, csize: u32, usize_: u32) -> Self {
        self.method = method;
        self.central = Sizes { csize, usize_ };
        self.local = self.central;
        self
    }

    /// Only the central directory claims these sizes.
    pub fn lie_central(mut self, csize: u32, usize_: u32) -> Self {
        self.central = Sizes { csize, usize_ };
        self
    }
}

fn le16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// The archive, with an end-of-central-directory record that can be altered by
/// the caller afterwards (it is the last 22 bytes).
pub(crate) fn build(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for e in entries {
        offsets.push(out.len() as u32);
        let name = e.local_name.as_ref().unwrap_or(&e.name);
        out.extend_from_slice(b"PK\x03\x04");
        le16(&mut out, 20);
        le16(&mut out, e.flags);
        le16(&mut out, e.method);
        le16(&mut out, 0);
        le16(&mut out, 0);
        // A data descriptor (bit 3) leaves the sizes and the CRC at zero.
        let descriptor = e.flags & 0x0008 != 0;
        le32(&mut out, if descriptor { 0 } else { crc32(&e.data) });
        le32(&mut out, if descriptor { 0 } else { e.local.csize });
        le32(&mut out, if descriptor { 0 } else { e.local.usize_ });
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(&e.data);
    }
    let cd_start = out.len() as u32;
    for (e, off) in entries.iter().zip(&offsets) {
        out.extend_from_slice(b"PK\x01\x02");
        le16(&mut out, 20);
        le16(&mut out, 20);
        le16(&mut out, e.flags);
        le16(&mut out, e.method);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le32(&mut out, crc32(&e.data));
        le32(&mut out, e.central.csize);
        le32(&mut out, e.central.usize_);
        le16(&mut out, e.name.len() as u16);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le32(&mut out, 0);
        le32(&mut out, e.offset_override.unwrap_or(*off));
        out.extend_from_slice(&e.name);
    }
    let cd_size = out.len() as u32 - cd_start;
    out.extend_from_slice(b"PK\x05\x06");
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, entries.len() as u16);
    le16(&mut out, entries.len() as u16);
    le32(&mut out, cd_size);
    le32(&mut out, cd_start);
    le16(&mut out, 0);
    out
}
