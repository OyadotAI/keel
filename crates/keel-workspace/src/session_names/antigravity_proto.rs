//! Verified Antigravity title fields; all other annotation and summary fields remain intact.
const TITLE: u64 = 1;
const ANNOTATIONS: u64 = 15;
const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const BYTES: u64 = 2;
const FIXED32: u64 = 5;
const FIXED64_BYTES: usize = 8;
const FIXED32_BYTES: usize = 4;
const VARINT_BITS: usize = 7;
const VARINT_MORE: u8 = 128;

/// The native summary duplicates its title inside ConversationAnnotations (field 15).
pub(super) fn summary(bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    rewrite(bytes, name, true)
}
fn rewrite(bytes: &[u8], name: &str, summary: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut annotated = false;
    while i < bytes.len() {
        let start = i;
        let key = varint(bytes, &mut i)?;
        let mut payload = None;
        if key >> 3 == 0 {
            return Err("Invalid summary field".into());
        }
        match key & 7 {
            VARINT => {
                varint(bytes, &mut i)?;
            }
            FIXED64 => i = i.saturating_add(FIXED64_BYTES),
            BYTES => {
                let len = usize::try_from(varint(bytes, &mut i)?).map_err(|e| e.to_string())?;
                let from = i;
                i = i.checked_add(len).ok_or("Invalid summary length")?;
                payload = Some(from..i);
            }
            FIXED32 => i = i.saturating_add(FIXED32_BYTES),
            _ => return Err("Unsupported summary encoding".into()),
        }
        if i > bytes.len() {
            return Err("Incomplete summary encoding".into());
        }
        if key >> 3 == TITLE {
            if key & 7 != BYTES {
                return Err("Unsupported title encoding".into());
            }
            continue;
        }
        if summary && key >> 3 == ANNOTATIONS {
            let range = payload.ok_or("Unsupported annotation encoding")?;
            field(&mut out, ANNOTATIONS, &rewrite(&bytes[range], name, false)?);
            annotated = true;
        } else {
            out.extend_from_slice(&bytes[start..i]);
        }
    }
    field(&mut out, TITLE, name.as_bytes());
    if summary && !annotated {
        field(&mut out, ANNOTATIONS, &rewrite(&[], name, false)?);
    }
    Ok(out)
}
fn varint(bytes: &[u8], i: &mut usize) -> Result<u64, String> {
    let mut value = 0;
    for shift in (0..u64::BITS as usize).step_by(VARINT_BITS) {
        let byte = *bytes.get(*i).ok_or("Incomplete summary encoding")?;
        *i += 1;
        if shift == u64::BITS as usize - 1 && byte > 1 {
            return Err("Invalid summary varint".into());
        }
        value |= u64::from(byte & 127) << shift;
        if byte < VARINT_MORE {
            return Ok(value);
        }
    }
    Err("Invalid summary varint".into())
}
fn push(out: &mut Vec<u8>, mut value: u64) {
    while value >= u64::from(VARINT_MORE) {
        out.push((value as u8 & 127) | VARINT_MORE);
        value >>= VARINT_BITS;
    }
    out.push(value as u8);
}
fn field(out: &mut Vec<u8>, key: u64, bytes: &[u8]) {
    push(out, (key << 3) | BYTES);
    push(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// Locate only the root title, including when native prototext puts tags on the same line.
pub(super) fn title(text: &str) -> Result<Option<(std::ops::Range<usize>, String)>, String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = quoted_end(bytes, i)?;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' | b'<' => {
                depth += 1;
                i += 1;
            }
            b'}' | b'>' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if depth != 0 || &text[start..i] != "title" {
                    continue;
                }
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if bytes.get(i) != Some(&b':') {
                    return Err("Unsupported Antigravity title annotation".into());
                }
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let end = quoted_end(bytes, i)?;
                let value = serde_json::from_str(&text[i..end])
                    .map_err(|_| "Unsupported Antigravity title annotation")?;
                return Ok(Some((start..end, value)));
            }
            _ => i += 1,
        }
    }
    Ok(None)
}
fn quoted_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    if bytes.get(start) != Some(&b'"') {
        return Err("Unsupported Antigravity title annotation".into());
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("Incomplete Antigravity title annotation".into())
}
