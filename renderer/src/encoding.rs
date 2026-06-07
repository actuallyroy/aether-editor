// Text-encoding registry for the status-bar encoding selector ("Reopen with
// Encoding"). One source of truth mapping a human label → an `encoding_rs`
// codec, used by both the quick-pick list and the actual decode.

use encoding_rs::Encoding;

/// The encodings offered in the picker, in display order. Each is (label, codec).
pub const ENCODINGS: &[(&str, &'static Encoding)] = &[
    ("UTF-8", encoding_rs::UTF_8),
    // ASCII has no encoding_rs codec; `decode` handles it specially (bytes ≥ 128
    // become U+FFFD). The codec here is an unused placeholder.
    ("ASCII", encoding_rs::UTF_8),
    ("UTF-16 LE", encoding_rs::UTF_16LE),
    ("UTF-16 BE", encoding_rs::UTF_16BE),
    ("Western (Windows 1252)", encoding_rs::WINDOWS_1252),
    ("Western (ISO 8859-1)", encoding_rs::WINDOWS_1252), // ISO-8859-1 maps to 1252 in the WHATWG set
    ("Central European (Windows 1250)", encoding_rs::WINDOWS_1250),
    ("Cyrillic (Windows 1251)", encoding_rs::WINDOWS_1251),
    ("Greek (Windows 1253)", encoding_rs::WINDOWS_1253),
    ("Turkish (Windows 1254)", encoding_rs::WINDOWS_1254),
    ("Japanese (Shift JIS)", encoding_rs::SHIFT_JIS),
    ("Japanese (EUC-JP)", encoding_rs::EUC_JP),
    ("Chinese (GBK)", encoding_rs::GBK),
    ("Chinese (Big5)", encoding_rs::BIG5),
    ("Korean (EUC-KR)", encoding_rs::EUC_KR),
];

/// Look up the codec for a picker label. Falls back to UTF-8 for unknown labels.
pub fn codec_for(label: &str) -> &'static Encoding {
    ENCODINGS
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, e)| *e)
        .unwrap_or(encoding_rs::UTF_8)
}

/// The `&'static` label matching `label` (for storing on a Document). Falls back
/// to "UTF-8". Lets a picker's owned String map back to a static label.
pub fn static_label(label: &str) -> &'static str {
    ENCODINGS
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(l, _)| *l)
        .unwrap_or("UTF-8")
}

/// Decode `bytes` with the named encoding, returning the text. Lossy (invalid
/// sequences become U+FFFD), so it always succeeds.
pub fn decode(label: &str, bytes: &[u8]) -> String {
    if label == "ASCII" {
        // 7-bit ASCII: pass through < 128, replace the rest.
        return bytes
            .iter()
            .map(|&b| if b < 128 { b as char } else { '\u{FFFD}' })
            .collect();
    }
    let (text, _, _) = codec_for(label).decode(bytes);
    text.into_owned()
}
