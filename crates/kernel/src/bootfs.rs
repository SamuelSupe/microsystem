const HEADER_BYTES: usize = 110;

pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

pub struct Archive<'a> {
    bytes: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a> Archive<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            finished: false,
        }
    }
}

impl<'a> Iterator for Archive<'a> {
    type Item = Result<Entry<'a>, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished || self.offset == self.bytes.len() {
            return None;
        }
        let Some(header) = self.bytes.get(self.offset..self.offset + HEADER_BYTES) else {
            self.finished = true;
            return Some(Err(()));
        };
        if &header[..6] != b"070701" {
            self.finished = true;
            return Some(Err(()));
        }
        let Some(file_bytes) = hex(&header[54..62]) else {
            self.finished = true;
            return Some(Err(()));
        };
        let Some(name_bytes) = hex(&header[94..102]) else {
            self.finished = true;
            return Some(Err(()));
        };
        if name_bytes == 0 {
            self.finished = true;
            return Some(Err(()));
        }
        let name_start = self.offset + HEADER_BYTES;
        let Some(name_end) = name_start.checked_add(name_bytes) else {
            self.finished = true;
            return Some(Err(()));
        };
        let Some(raw_name) = self.bytes.get(name_start..name_end) else {
            self.finished = true;
            return Some(Err(()));
        };
        let Some(name) = raw_name
            .strip_suffix(&[0])
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
        else {
            self.finished = true;
            return Some(Err(()));
        };
        let data_start = align4(name_end);
        let Some(data_end) = data_start.checked_add(file_bytes) else {
            self.finished = true;
            return Some(Err(()));
        };
        let Some(data) = self.bytes.get(data_start..data_end) else {
            self.finished = true;
            return Some(Err(()));
        };
        self.offset = align4(data_end);
        if name == "TRAILER!!!" {
            self.finished = true;
            return None;
        }
        Some(Ok(Entry { name, data }))
    }
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn hex(bytes: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    for &byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit as usize)?;
    }
    Some(value)
}
