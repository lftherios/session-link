//! Images. A harness records one as base64, as a data URL or as a link. The
//! `session/v0` part carries a URL either way: the viewer shows a data URL as
//! it is and never fetches a link without a click.

use serde_json::{Value, json};

/// The part for an image recorded as base64. `mime` is the harness's own
/// label; where it gave none, the image's first bytes say. An image whose
/// kind cannot be told is not turned into a part.
pub(crate) fn image_part(data: &str, mime: &str) -> Option<Value> {
    if data.is_empty() {
        return None;
    }
    let mime = if mime.is_empty() { sniff(data)? } else { mime };
    Some(json!({"type": "image", "url": format!("data:{mime};base64,{data}"), "mime": mime}))
}

/// The kind of an image from the first bytes its base64 spells.
fn sniff(data: &str) -> Option<&'static str> {
    let head = data.as_bytes().get(..24.min(data.len()))?;
    let raw = decode(head);
    if raw.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if raw.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if raw.starts_with(b"GIF8") {
        Some("image/gif")
    } else if raw.len() >= 12 && &raw[..4] == b"RIFF" && &raw[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Standard base64, as far as Go's decoder reads it before it stops: line
/// breaks are passed over, and bytes come only from a whole group of four
/// characters, the last of which may be padded.
fn decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0_u8; 4];
    let mut filled = 0;
    let mut rest = text
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'));
    while let Some(byte) = rest.next() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            // Padding closes a group that has two or three characters, the
            // shorter one with a second `=`, and nothing is read after it.
            b'=' if filled == 3 || (filled == 2 && rest.next() == Some(b'=')) => {
                let bits = u32::from(group[0]) << 18
                    | u32::from(group[1]) << 12
                    | u32::from(group[2]) << 6;
                out.extend_from_slice(&bits.to_be_bytes()[1..filled]);
                break;
            }
            _ => break,
        };
        group[filled] = value;
        filled += 1;
        if filled == 4 {
            let bits = group
                .iter()
                .fold(0_u32, |bits, value| (bits << 6) | u32::from(*value));
            out.extend_from_slice(&bits.to_be_bytes()[1..]);
            group = [0; 4];
            filled = 0;
        }
    }
    out
}

/// The part for an image recorded as a URL: a data URL holding the image
/// itself, or a link.
pub(crate) fn image_url_part(url: &str) -> Option<Value> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (mime, _) = rest.split_once(';')?;
        return mime
            .starts_with("image/")
            .then(|| json!({"type": "image", "url": url, "mime": mime}));
    }
    (url.starts_with("http://") || url.starts_with("https://"))
        .then(|| json!({"type": "image", "url": url}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_without_a_label_is_told_by_its_first_bytes() {
        // The first bytes of a PNG, a JPEG, a GIF and a WebP, as base64.
        assert_eq!(
            image_part("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB", "").unwrap()["mime"],
            "image/png"
        );
        assert_eq!(
            image_part("/9j/4AAQSkZJRgABAQAAAQABAAD/2w", "").unwrap()["mime"],
            "image/jpeg"
        );
        assert_eq!(
            image_part("R0lGODlhAQABAIAAAAAAAP///yH5BAEA", "").unwrap()["mime"],
            "image/gif"
        );
        assert_eq!(
            image_part("UklGRhoAAABXRUJQVlA4TA0AAAAvAAAA", "").unwrap()["mime"],
            "image/webp"
        );
        assert_eq!(image_part("aGVsbG8gd29ybGQ=", ""), None);
        assert_eq!(image_part("", "image/png"), None);
        let labelled = image_part("aGVsbG8=", "image/png").unwrap();
        assert_eq!(labelled["url"], "data:image/png;base64,aGVsbG8=");
    }

    #[test]
    fn an_image_by_url_is_a_data_url_or_a_link() {
        assert_eq!(
            image_url_part("data:image/png;base64,AAAA").unwrap()["mime"],
            "image/png"
        );
        assert_eq!(
            image_url_part("https://example.test/a.png").unwrap(),
            json!({"type": "image", "url": "https://example.test/a.png"})
        );
        assert_eq!(image_url_part("data:text/plain;base64,AAAA"), None);
        assert_eq!(image_url_part("data:image/png"), None);
        assert_eq!(image_url_part("file:///a.png"), None);
    }
}
