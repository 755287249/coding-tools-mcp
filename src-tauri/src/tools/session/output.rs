use serde_json::Value;

const SESSION_BUFFER_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    Delta,
    Tail,
    All,
    None,
    Summary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventDetail {
    Compact,
    Full,
}

impl EventDetail {
    fn parse(value: Option<&str>, default: Self) -> Self {
        match value {
            Some("compact") => Self::Compact,
            Some("full") => Self::Full,
            _ => default,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Full => "full",
        }
    }
}

impl OutputMode {
    fn parse(value: Option<&str>, default: Self) -> Self {
        match value {
            Some("delta") => Self::Delta,
            Some("tail") => Self::Tail,
            Some("all") => Self::All,
            Some("none") => Self::None,
            Some("summary") => Self::Summary,
            _ => default,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Tail => "tail",
            Self::All => "all",
            Self::None => "none",
            Self::Summary => "summary",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OutputOptions {
    pub mode: OutputMode,
    pub event_detail: EventDetail,
    pub cursor: u64,
    pub max_output_bytes: usize,
    pub tail_lines: usize,
}

impl OutputOptions {
    pub fn from_args(args: &Value, default_mode: OutputMode) -> Self {
        Self {
            mode: OutputMode::parse(
                args.get("output_mode").and_then(Value::as_str),
                default_mode,
            ),
            event_detail: EventDetail::parse(
                args.get("event_detail").and_then(Value::as_str),
                EventDetail::Full,
            ),
            cursor: args.get("cursor").and_then(Value::as_u64).unwrap_or(0),
            max_output_bytes: args
                .get("max_output_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(65_536)
                .clamp(1, 1_048_576) as usize,
            tail_lines: args
                .get("tail_lines")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .clamp(1, 10_000) as usize,
        }
    }

    pub fn tail(max_output_bytes: usize) -> Self {
        Self {
            mode: OutputMode::Tail,
            event_detail: EventDetail::Full,
            cursor: 0,
            max_output_bytes,
            tail_lines: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ProcessOutputEncoding {
    #[default]
    Unknown,
    Utf16Le,
    Utf16Be,
}

impl ProcessOutputEncoding {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "utf-8",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
        }
    }

    fn is_utf16(self) -> bool {
        matches!(self, Self::Utf16Le | Self::Utf16Be)
    }
}

#[derive(Debug, Default)]
pub(super) struct ProcessOutputStream {
    pub(super) data: Vec<u8>,
    pub(super) total_bytes: usize,
    pub(super) encoding: ProcessOutputEncoding,
}

#[derive(Clone, Debug)]
pub(super) struct ProcessOutputSnapshot {
    pub(super) data: Vec<u8>,
    pub(super) total_bytes: usize,
    pub(super) encoding: ProcessOutputEncoding,
}

impl ProcessOutputStream {
    pub(super) fn append(&mut self, chunk: &[u8]) -> (usize, Vec<u8>) {
        let stream_offset = self.total_bytes;
        let retained_start = self.total_bytes.saturating_sub(self.data.len());
        let previous_len = self.data.len();
        self.data.extend_from_slice(chunk);
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if self.encoding == ProcessOutputEncoding::Unknown {
            self.encoding = detect_process_output_encoding(&self.data);
        }
        let prefix =
            process_output_prefix(&self.data[..previous_len], self.encoding, retained_start);
        trim_process_buffer(
            &mut self.data,
            SESSION_BUFFER_BYTES,
            self.encoding,
            self.total_bytes,
        );
        (stream_offset, prefix)
    }

    pub(super) fn snapshot(&self) -> ProcessOutputSnapshot {
        ProcessOutputSnapshot {
            data: self.data.clone(),
            total_bytes: self.total_bytes,
            encoding: self.encoding,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct OutputEvent {
    pub(super) sequence: u64,
    pub(super) stream: &'static str,
    pub(super) stream_offset: usize,
    pub(super) prefix: Vec<u8>,
    pub(super) data: Vec<u8>,
}

fn decode_utf16_unit(pair: &[u8], encoding: ProcessOutputEncoding) -> u16 {
    if encoding == ProcessOutputEncoding::Utf16Le {
        u16::from_le_bytes([pair[0], pair[1]])
    } else {
        u16::from_be_bytes([pair[0], pair[1]])
    }
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

pub(super) fn trim_process_buffer(
    buf: &mut Vec<u8>,
    limit: usize,
    encoding: ProcessOutputEncoding,
    total_bytes: usize,
) {
    if buf.len() <= limit {
        return;
    }

    let retained_start = total_bytes.saturating_sub(buf.len());
    let mut drop = buf.len() - limit;
    if encoding.is_utf16() {
        if (retained_start + drop) % 2 != 0 {
            drop = drop.saturating_add(1);
        }
        if drop + 1 < buf.len() {
            let unit = decode_utf16_unit(&buf[drop..drop + 2], encoding);
            if (0xDC00..=0xDFFF).contains(&unit) {
                drop = drop.saturating_add(2);
            }
        }
    } else {
        while drop < buf.len() && is_utf8_continuation(buf[drop]) {
            drop += 1;
        }
    }
    buf.drain(..drop.min(buf.len()));
}

pub(super) struct Truncated {
    pub(super) content: String,
    pub(super) truncated: bool,
}

fn detect_process_output_encoding(bytes: &[u8]) -> ProcessOutputEncoding {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return ProcessOutputEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return ProcessOutputEncoding::Utf16Be;
    }

    let sample = &bytes[..bytes.len().min(4096)];
    let pairs = sample.len() / 2;
    let (even_nuls, odd_nuls) =
        sample
            .chunks_exact(2)
            .fold((0_usize, 0_usize), |(even, odd), pair| {
                (
                    even + usize::from(pair[0] == 0),
                    odd + usize::from(pair[1] == 0),
                )
            });
    if pairs >= 2 && odd_nuls * 3 >= pairs && even_nuls * 10 <= pairs {
        ProcessOutputEncoding::Utf16Le
    } else if pairs >= 2 && even_nuls * 3 >= pairs && odd_nuls * 10 <= pairs {
        ProcessOutputEncoding::Utf16Be
    } else {
        ProcessOutputEncoding::Unknown
    }
}

pub(super) fn process_output_prefix(
    previous: &[u8],
    encoding: ProcessOutputEncoding,
    absolute_start: usize,
) -> Vec<u8> {
    if previous.is_empty() {
        return Vec::new();
    }
    if encoding.is_utf16() {
        let absolute_end = absolute_start.saturating_add(previous.len());
        let dangling_byte = absolute_end % 2;
        let complete_end = previous.len().saturating_sub(dangling_byte);
        let mut count = dangling_byte;
        if complete_end >= 2 {
            let unit = decode_utf16_unit(&previous[complete_end - 2..complete_end], encoding);
            if (0xD800..=0xDBFF).contains(&unit) {
                count = count.saturating_add(2);
            }
        }
        return previous[previous.len().saturating_sub(count)..].to_vec();
    }

    let start = previous.len().saturating_sub(4);
    let tail = &previous[start..];
    for index in 0..tail.len() {
        if std::str::from_utf8(&tail[index..]).is_ok() {
            return Vec::new();
        }
        if let Err(error) = std::str::from_utf8(&tail[index..]) {
            if error.error_len().is_none() && error.valid_up_to() == 0 {
                return tail[index..].to_vec();
            }
        }
    }
    Vec::new()
}

pub(super) fn decode_process_output_with_encoding(
    bytes: &[u8],
    encoding: ProcessOutputEncoding,
) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let encoding = if encoding == ProcessOutputEncoding::Unknown {
        detect_process_output_encoding(bytes)
    } else {
        encoding
    };
    if encoding.is_utf16() {
        let payload = if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
            &bytes[2..]
        } else {
            bytes
        };
        let units = payload
            .chunks_exact(2)
            .map(|pair| {
                if encoding == ProcessOutputEncoding::Utf16Le {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        decode_narrow_output(bytes)
    }
}

/// UTF-8 first; otherwise the console's legacy code page. Windows tools such
/// as `cmd /c dir`, `ipconfig` or a piped Python write OEM/ANSI bytes (GBK on
/// Chinese systems), which would otherwise come back as mojibake.
fn decode_narrow_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => decode_legacy_code_page(bytes)
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned()),
    }
}

#[cfg(windows)]
fn decode_legacy_code_page(bytes: &[u8]) -> Option<String> {
    use windows::Win32::Globalization::{GetOEMCP, MultiByteToWideChar, MB_ERR_INVALID_CHARS};

    fn strict(code_page: u32, bytes: &[u8]) -> Option<String> {
        if bytes.is_empty() {
            return Some(String::new());
        }
        if i32::try_from(bytes.len()).is_err() {
            return None;
        }
        // SAFETY: both buffers are valid slices; the call only reads `bytes`
        // and writes at most `wide.len()` UTF-16 units.
        let needed = unsafe { MultiByteToWideChar(code_page, MB_ERR_INVALID_CHARS, bytes, None) };
        if needed <= 0 {
            return None;
        }
        let mut wide = vec![0u16; needed as usize];
        let written =
            unsafe { MultiByteToWideChar(code_page, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide)) };
        if written <= 0 {
            return None;
        }
        wide.truncate(written as usize);
        Some(String::from_utf16_lossy(&wide))
    }

    // SAFETY: GetOEMCP has no preconditions.
    let code_page = unsafe { GetOEMCP() };
    if code_page == 65001 {
        return None;
    }
    strict(code_page, bytes).or_else(|| {
        // A chunk may end in the middle of a double-byte character.
        let (&last, rest) = bytes.split_last()?;
        (last >= 0x80)
            .then(|| strict(code_page, rest))
            .flatten()
            .map(|mut text| {
                text.push('\u{FFFD}');
                text
            })
    })
}

#[cfg(not(windows))]
fn decode_legacy_code_page(_bytes: &[u8]) -> Option<String> {
    None
}

pub(super) fn complete_output_boundary(bytes: &[u8], encoding: ProcessOutputEncoding) -> usize {
    if encoding.is_utf16() {
        let mut end = bytes.len() - (bytes.len() % 2);
        if end >= 2 {
            let unit = decode_utf16_unit(&bytes[end - 2..end], encoding);
            if (0xD800..=0xDBFF).contains(&unit) {
                end -= 2;
            }
        }
        return end;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => bytes.len(),
    }
}

pub(super) fn align_output_start(
    data: &[u8],
    mut offset: usize,
    encoding: ProcessOutputEncoding,
    retained_start: usize,
) -> usize {
    offset = offset.min(data.len());
    if encoding.is_utf16() {
        if (retained_start + offset) % 2 != 0 {
            offset = offset.saturating_sub(1);
        }
        if offset >= 2 && offset + 1 < data.len() {
            let unit = decode_utf16_unit(&data[offset..offset + 2], encoding);
            if (0xDC00..=0xDFFF).contains(&unit) {
                offset -= 2;
            }
        }
    } else {
        while offset > 0 && offset < data.len() && is_utf8_continuation(data[offset]) {
            offset -= 1;
        }
    }
    offset
}

pub(super) fn bounded_output_end(
    data: &[u8],
    start: usize,
    limit: usize,
    encoding: ProcessOutputEncoding,
) -> usize {
    if start >= data.len() {
        return start.min(data.len());
    }
    let requested_end = data.len().min(start.saturating_add(limit));
    let complete = complete_output_boundary(&data[start..requested_end], encoding);
    if complete > 0 {
        return start + complete;
    }

    let expanded_end = data.len().min(requested_end.saturating_add(4));
    for candidate_end in requested_end.saturating_add(1)..=expanded_end {
        let expanded_complete = complete_output_boundary(&data[start..candidate_end], encoding);
        if expanded_complete > 0 {
            return start + expanded_complete;
        }
    }
    requested_end.max(start + 1).min(data.len())
}

#[cfg(test)]
pub(super) fn decode_process_output(bytes: &[u8]) -> String {
    decode_process_output_with_encoding(bytes, ProcessOutputEncoding::Unknown)
}

fn decode_complete_process_output(bytes: &[u8], encoding: ProcessOutputEncoding) -> String {
    let complete = complete_output_boundary(bytes, encoding);
    decode_process_output_with_encoding(&bytes[..complete], encoding)
}

pub(super) fn decode_output_event(event: &OutputEvent, encoding: ProcessOutputEncoding) -> String {
    let mut bytes = event.prefix.clone();
    bytes.extend_from_slice(&event.data);
    decode_complete_process_output(&bytes, encoding)
}

fn truncate_decoded_tail(decoded: String, max_bytes: usize) -> Truncated {
    let truncated = decoded.len() > max_bytes;
    let mut start = decoded.len().saturating_sub(max_bytes);
    while start < decoded.len() && !decoded.is_char_boundary(start) {
        start += 1;
    }
    Truncated {
        content: decoded[start..].to_string(),
        truncated,
    }
}

pub(super) fn truncate_tail(
    bytes: &[u8],
    max_bytes: usize,
    encoding: ProcessOutputEncoding,
) -> Truncated {
    truncate_decoded_tail(decode_complete_process_output(bytes, encoding), max_bytes)
}

pub(super) fn summarize_stream(
    bytes: &[u8],
    max_bytes: usize,
    tail_lines: usize,
    encoding: ProcessOutputEncoding,
) -> Truncated {
    let source = decode_complete_process_output(bytes, encoding);
    let mut lines = Vec::<String>::new();
    for line in source
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
    {
        if lines.last().is_some_and(|previous| previous == line) {
            continue;
        }
        lines.push(line.to_string());
    }
    let start = lines.len().saturating_sub(tail_lines);
    let summary = lines[start..].join("\n");
    truncate_decoded_tail(summary, max_bytes)
}
