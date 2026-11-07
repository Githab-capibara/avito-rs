//! Экстрактор `v2_html`: порт `find_json_on_page` из `parser_cls.py`.
//!
//! Авито отдаёт состояние страницы в `<script type="mime/invalid"
//! data-mfe-state="true">` (HTML-эскейпы внутри). JS object → JSON через
//! `find_items_node`, дальше та же нормализация в [`super::v1_json`].

use super::v1_json::V1JsonExtractor;
use super::{ExtractError, Extractor, ExtractorKind};
use crate::dto::Listing;
use crate::providers::FetchedPage;

/// Селектор оригинального скрипта состояния.
const SCRIPT_MARKER: &str = "mime/invalid";

/// Минимальная длина HTML для разбора (пустая страница — не страница).
const MIN_HTML_LEN: usize = 64;

/// CSS-подобный парсер `<script type="..." data-mfe-state="true">`.
///
/// `scraper` в оригинале строит DOM; здесь достаточно линейного прохода —
/// линейный разбор дешевле и не требует разбора всего документа.
fn find_state_script(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find("<script") {
        let start = cursor + rel;
        let open_end = lower[start..].find('>')? + start;
        let tag = &lower[start..=open_end];
        let is_state = tag.contains(SCRIPT_MARKER)
            && tag.contains("data-mfe-state")
            && !tag.contains("sandbox");
        if !is_state {
            cursor = open_end + 1;
            continue;
        }
        let close = lower[open_end..].find("</script")? + open_end;
        let body = html.get((open_end + 1)..close)?;
        if body.trim().is_empty() {
            return None;
        }
        return Some(body);
    }
    None
}

/// HTML-unescape (порт `html.unescape` из оригинального BS4-пути).
fn html_unescape(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            // копируем исходный символ (utf-8 безопасно: сурогаты не встречаются)
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        let rest = &raw[i..];
        let semi = match rest.find(';') {
            Some(s) if s <= 10 => s,
            _ => {
                out.push('&');
                i += 1;
                continue;
            }
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00a0}'),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                i += semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// Декодировать один JS-строковый литерал (escapes внутри `<script>`).
fn decode_js_string(raw: &str) -> Option<String> {
    // Обрабатываем \\ -> \, \" -> ", \/ -> /, \n \t \uXXXX
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            '/' => out.push('/'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                let n = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// Порт `_extract_api_catalog`: вытащить из HTML JSON-состояние и найти
/// `result.catalog.items` → `Vec<Listing>` через `v1_json`.
pub fn extract_state(html: &str) -> Result<Vec<Listing>, ExtractError> {
    if html.len() < MIN_HTML_LEN {
        return Err(ExtractError::NoJson);
    }
    let script = match find_state_script(html) {
        Some(s) => s,
        None => return Err(ExtractError::NoJson),
    };
    let unescaped = html_unescape(script);
    let value: serde_json::Value = serde_json::from_str(unescaped.trim())
        .map_err(|e| ExtractError::MalformedJson(e.to_string()))?;
    let origin = "<script data-mfe-state>";
    // Оригинал: data.get("i18n", {}).get("hasMessages") →
    // data.get("loaderData", {}).get("data", {})
    if !value
        .pointer("/i18n/hasMessages")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(ExtractError::NoItems {
            origin: "i18n.hasMessages=false".to_owned(),
        });
    }
    let state = match value.pointer("/loaderData/data") {
        Some(s) => s,
        None => {
            return Err(ExtractError::NoItems {
                origin: origin.to_owned(),
            })
        }
    };
    match super::find_items_node(state) {
        Some(items) => V1JsonExtractor
            .extract_items(items)
            .map_err(|_| ExtractError::NoItems {
                origin: origin.to_owned(),
            }),
        None => Err(ExtractError::NoItems {
            origin: origin.to_owned(),
        }),
    }
}

/// Экстрактор HTML-состояния.
#[derive(Debug, Clone, Copy, Default)]
pub struct V2HtmlExtractor;

impl Extractor for V2HtmlExtractor {
    fn kind(&self) -> ExtractorKind {
        ExtractorKind::V2Html
    }
    fn extract(&self, page: &FetchedPage) -> Result<Vec<Listing>, ExtractError> {
        if let Some(body) = &page.body {
            if let Some(text) = body.as_str() {
                if let Ok(listings) = extract_state(text) {
                    return Ok(listings);
                }
            }
        }
        let html = String::from_utf8_lossy(&page.raw_body);
        extract_state(&html)
    }
}

impl V2HtmlExtractor {
    /// Декодировать JS-строку (публично для тестов/диагностики).
    #[must_use]
    pub fn decode_string(raw: &str) -> Option<String> {
        decode_js_string(raw)
    }
    /// Извлечь JSON-состояние из HTML (публично для тестов).
    ///
    /// # Errors
    /// Нет скрипта / скрипт без `hasMessages` / битый JSON.
    pub fn state(html: &str) -> Result<serde_json::Value, ExtractError> {
        let script = find_state_script(html).ok_or(ExtractError::NoJson)?;
        let unescaped = html_unescape(script);
        serde_json::from_str(unescaped.trim())
            .map_err(|e| ExtractError::MalformedJson(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_state_script_tag() {
        let html = r#"<html><body>
            <script type="mime/invalid" data-mfe-state="true">{"a":1}</script>
            <script>var x = 2;</script>
        </body></html>"#;
        assert_eq!(find_state_script(html), Some("{\"a\":1}"));
    }
    #[test]
    fn skips_plain_scripts_and_sandbox_tag() {
        let plain = r#"<script>var x = 2;</script>"#;
        assert!(find_state_script(plain).is_none());
        // mime-тип есть, но data-mfe-state отсутствует
        let no_state = r#"<script type="mime/invalid">{"a":1}</script>"#;
        assert!(find_state_script(no_state).is_none());
        // пустой скрипт состояния → нет
        let empty = r#"<script type="mime/invalid" data-mfe-state="true"></script>"#;
        assert!(find_state_script(empty).is_none());
    }
    #[test]
    fn unescapes_html_entities() {
        assert_eq!(
            html_unescape("a&amp;b &lt;c&gt; &quot;d&quot;"),
            "a&b <c> \"d\""
        );
        assert_eq!(html_unescape("&#72;ello &#x69;"), "Hello i");
        assert_eq!(html_unescape("plain"), "plain");
        assert_eq!(html_unescape("bad &unk; entity"), "bad &unk; entity");
    }
    #[test]
    fn decodes_js_escapes() {
        assert_eq!(decode_js_string("a\\nb").as_deref(), Some("a\nb"));
        assert_eq!(decode_js_string("\\u0041b").as_deref(), Some("Ab"));
        assert_eq!(decode_js_string("a\\\"b").as_deref(), Some("a\"b"));
    }
    #[test]
    fn requires_has_messages_flag() {
        let html = r#"<script type="mime/invalid" data-mfe-state="true">{"i18n":{"hasMessages":false}}</script>"#;
        assert!(matches!(
            extract_state(html),
            Err(ExtractError::NoItems { .. })
        ));
    }
    #[test]
    fn extracts_items_through_state_node() {
        let state = serde_json::json!({
            "i18n": {"hasMessages": true},
            "loaderData": {"data": {"catalog": {"items": [
                {"id": 123, "title": "x", "urlPath": "/i/x",
                 "priceDetailed": {"value": 100}}
            ]}}}
        });
        let html = format!(
            "<script type=\"mime/invalid\" data-mfe-state=\"true\">{}</script>",
            serde_json::to_string(&state).unwrap()
        );
        let ads = extract_state(&html).expect("extract");
        assert_eq!(ads.len(), 1);
        assert_eq!(ads[0].id, "123");
        assert_eq!(ads[0].price, Some(100));
    }
}
