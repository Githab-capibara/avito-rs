//! Экспорт результатов: CSV всегда, XLSX за feature `xlsx`.
//!
//! Порт `parser/export/` оригинала: стратегия именования + запись после
//! каждой страницы, чтобы прогресс не терялся.

pub mod csv;

#[cfg(feature = "xlsx")]
pub mod xlsx;

use anyhow::Result;

use crate::dto::Listing;

/// Трейт-порт хранилища результатов (Excel/CSV/API в оригинале).
pub trait ResultExporter: Send + Sync + std::fmt::Debug {
    /// Сохранить пачку объявлений.
    fn save(&self, ads: &[Listing]) -> Result<()>;
    /// Куда пишет (для логов/статуса).
    fn target(&self) -> String;
}

/// CSV-заголовок: все поля объявления.
pub const CSV_HEADER: &[&str] = &[
    "id",
    "title",
    "description",
    "price",
    "url",
    "seller",
    "address",
    "total_views",
    "today_views",
    "reserved",
    "promoted",
    "sort_ts_ms",
];

/// Экранирование значения CSV (RFC 4180): кавычки, запятые, переводы строк.
#[must_use]
pub fn csv_escape(value: &str) -> String {
    let needs_quote = value
        .chars()
        .any(|c| c == ',' || c == '"' || c == '\n' || c == '\r');
    if needs_quote {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// Преобразовать объявление в строку CSV (без переводов строк).
#[must_use]
pub fn csv_row(ad: &Listing) -> String {
    let cells = [
        ad.id.clone(),
        ad.title.clone().unwrap_or_default(),
        ad.description
            .clone()
            .unwrap_or_default()
            .replace(['\n', '\r'], " "),
        ad.price.map(|p| p.to_string()).unwrap_or_default(),
        ad.url(),
        ad.seller_id.clone().unwrap_or_default(),
        ad.formatted_address.clone().unwrap_or_default(),
        ad.total_views.map(|v| v.to_string()).unwrap_or_default(),
        ad.today_views.map(|v| v.to_string()).unwrap_or_default(),
        ad.is_reserved.to_string(),
        ad.is_promotion.to_string(),
        ad.sort_time_stamp
            .map(|t| t.to_string())
            .unwrap_or_default(),
    ];
    cells
        .iter()
        .map(|c| csv_escape(c))
        .collect::<Vec<_>>()
        .join(",")
}

/// Строка заголовка CSV.
#[must_use]
pub fn csv_header_line() -> String {
    CSV_HEADER.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_quotes_and_commas() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line\nbreak"), "\"line\nbreak\"");
    }

    /// Split CSV строки с учётом кавычек (RFC 4180).
    fn split_quoted(row: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        let mut chars = row.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if in_quotes => {
                    if chars.peek() == Some(&'"') {
                        cur.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                }
                '"' => in_quotes = true,
                ',' if !in_quotes => {
                    out.push(std::mem::take(&mut cur));
                }
                other => cur.push(other),
            }
        }
        out.push(cur);
        out
    }

    #[test]
    fn row_has_header_width() {
        let ad = Listing {
            id: "1".to_owned(),
            title: Some("Test, item".to_owned()),
            description: Some("multi\nline".to_owned()),
            price: Some(100),
            ..Listing::default()
        };
        let row = csv_row(&ad);
        assert_eq!(split_quoted(&row).len(), CSV_HEADER.len());
        assert!(row.starts_with("1,\"Test, item\""));
        assert!(row.contains("multi line"), "newlines collapsed");
    }
}
