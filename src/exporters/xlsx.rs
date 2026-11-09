//! XLSX-экспорт — порт `parser/export/excel.py`.
//!
//! 14 колонок оригинала, защита от «формульной инъекции» (`excel_safe`),
//! локальная таймзона для даты публикации, append в существующий файл.

use anyhow::{Context, Result};
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::ResultExporter;
use crate::dto::Listing;

/// Заголовки колонок оригинала (`ExcelStorage.headers`).
pub const XLSX_HEADERS: [&str; 14] = [
    "Название",
    "Цена",
    "URL",
    "Описание",
    "Дата публикации",
    "Продавец",
    "Адрес",
    "Адрес пользователя",
    "Координаты",
    "Изображения",
    "Поднято",
    "Просмотры (всего)",
    "Просмотры (сегодня)",
    "Телефон",
];

/// Экранирование «формульной инъекции» (порт `excel_safe`):
/// ведущий `= + - @` заменяется на `'` + символ.
#[must_use]
pub fn excel_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 1);
    for (i, c) in value.chars().enumerate() {
        if i == 0 && matches!(c, '=' | '+' | '-' | '@') {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

/// `sortTimeStamp` (мс) → `dd.mm.yyyy hh:mm` в локальной таймзоне.
///
/// Оригинал использует `tzlocal.get_localzone()`; здесь — системный offset.
#[must_use]
pub fn ad_time(ts_ms: Option<i64>) -> String {
    let Some(ms) = ts_ms else {
        return String::new();
    };
    let secs = ms / 1000;
    let offset_min = local_offset_minutes();
    let local = secs + i64::from(offset_min) * 60;
    let days = local.div_euclid(86_400);
    let tod = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{d:02}.{m:02}.{y:04} {hh:02}:{mm:02}:{ss:02}")
}

/// Смещение локальной таймзоны в минутах (упрощённый порт `get_localzone`).
///
/// Точная локальная зона требует `libc::localtime_r` или крейта `chrono`;
/// здесь достаточно значения из `TZ` для колонки «Дата публикации».
/// Без `TZ` — UTC (0).
fn local_offset_minutes() -> i32 {
    lookup_offset(std::env::var("TZ").unwrap_or_default()).unwrap_or(0)
}

/// Разбор оффсета из строки TZ в минутах.
///
/// POSIX-нотация инвертирована относительно привычной: `UTC-3` означает
/// «на 3 часа впереди UTC» (исторически «+3» означало бы «позади»), поэтому
/// минус умножается на -60.
fn lookup_offset(tz: String) -> Option<i32> {
    let digits: String = tz
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
        .collect();
    digits.parse::<i32>().ok().map(|h| h * -60)
}

/// Гражданская дата из числа дней от 1970-01-01 (алгоритм Ходжкисона).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Порт `_get_item_coords`.
#[must_use]
pub fn coords(ad: &Listing) -> String {
    ad.coords
        .as_ref()
        .map(|c| {
            let lat = c.get("lat").and_then(|v| v.as_f64());
            let lng = c.get("lng").and_then(|v| v.as_f64());
            match (lat, lng) {
                (Some(a), Some(o)) => format!("{a};{o}"),
                _ => String::new(),
            }
        })
        .unwrap_or_default()
}

/// Порт `_get_item_address_user`.
#[must_use]
pub fn address_user(ad: &Listing) -> String {
    ad.coords
        .as_ref()
        .and_then(|c| c.get("address_user"))
        .map(|v| v.as_str().unwrap_or("").to_owned())
        .unwrap_or_default()
}

/// Изображения через `;` (порт `_get_image_urls`).
#[must_use]
pub fn images(ad: &Listing) -> String {
    ad.images.join(";")
}

/// XLSX-хранилище: создаёт книгу при первом сохранении, дописывает дальше.
#[derive(Debug)]
pub struct XlsxExporter {
    path: std::path::PathBuf,
}

impl XlsxExporter {
    /// Новый экспортёр для конкретного файла.
    #[must_use]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
    /// Путь к JSON-сайдкару с уже записанными строками.
    fn sidecar_path(&self) -> std::path::PathBuf {
        self.path.with_extension("json")
    }
    /// Создать книгу с шапкой, если файла ещё нет, и пустой сайдкар со строками.
    fn ensure_file(&self) -> Result<()> {
        if self.path.exists() && self.sidecar_path().exists() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create out dir {}", parent.display()))?;
            }
        }
        let name = self.path.to_string_lossy().into_owned();
        let mut workbook = Workbook::new(&name);
        let sheet = workbook.add_worksheet();
        let format = Format::new().set_bold();
        for (col, head) in XLSX_HEADERS.iter().enumerate() {
            sheet.write_string(0, col as u16, head, &format)?;
        }
        workbook.close().map_err(xlsx_err)?;
        let sidecar = self.sidecar_path();
        if !sidecar.exists() {
            std::fs::write(&sidecar, "[]")
                .with_context(|| format!("write sidecar {}", sidecar.display()))?;
        }
        Ok(())
    }
    /// Дописать строки: старые читаются из JSON-сайдкара (xlsx-запись нечитаема без zip-крейта),
    /// книга пересобирается из старых + новых строк, сайдкар обновляется.
    fn append_rows(&self, ads: &[Listing]) -> Result<()> {
        let sidecar = self.sidecar_path();
        let old: Vec<Vec<String>> = match std::fs::read_to_string(&sidecar) {
            Ok(t) => serde_json::from_str(&t)
                .context("parse xlsx sidecar")
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let new_rows: Vec<Vec<String>> = ads.iter().map(row_cells).collect();
        self.rewrite_with(old.clone(), ads)?;
        let mut all = old;
        all.extend(new_rows);
        std::fs::write(&sidecar, serde_json::to_string(&all)?)
            .with_context(|| format!("write sidecar {}", sidecar.display()))?;
        Ok(())
    }
    /// Пересобрать книгу целиком (шапка + старые строки + новые).
    fn rewrite_with(&self, old: Vec<Vec<String>>, new: &[Listing]) -> Result<()> {
        let name = self.path.to_string_lossy().into_owned();
        let mut workbook = Workbook::new(&name);
        let sheet = workbook.add_worksheet();
        let format = Format::new().set_bold();
        for (col, head) in XLSX_HEADERS.iter().enumerate() {
            sheet.write_string(0, col as u16, head, &format)?;
        }
        let mut row: u32 = 1;
        for cells in &old {
            for (col, value) in cells.iter().enumerate() {
                if value.is_empty() {
                    continue;
                }
                sheet.write_string(row, col as u16, value, &Format::new())?;
            }
            row += 1;
        }
        for ad in new {
            let cells = row_cells(ad);
            for (col, value) in cells.iter().enumerate() {
                if value.is_empty() {
                    continue;
                }
                sheet.write_string(row, col as u16, value, &Format::new())?;
            }
            row += 1;
        }
        workbook.close().map_err(xlsx_err)?;
        Ok(())
    }
}

/// Ошибка rust_xlsxwriter → anyhow.
fn xlsx_err(e: XlsxError) -> anyhow::Error {
    anyhow::anyhow!("xlsx: {e}")
}

/// Строка объявления в 14 колонок оригинала.
#[must_use]
pub fn row_cells(ad: &Listing) -> Vec<String> {
    let item_url = ad.url();
    vec![
        excel_safe(ad.title.as_deref().unwrap_or("")),
        ad.price.map(|p| p.to_string()).unwrap_or_default(),
        excel_safe(&item_url),
        excel_safe(ad.description.as_deref().unwrap_or("")),
        ad_time(ad.sort_time_stamp),
        excel_safe(ad.seller_id.as_deref().unwrap_or("")),
        excel_safe(ad.formatted_address.as_deref().unwrap_or("")),
        excel_safe(&address_user(ad)),
        excel_safe(&coords(ad)),
        excel_safe(&images(ad)),
        if ad.is_promotion {
            "Да".to_owned()
        } else {
            "Нет".to_owned()
        },
        ad.total_views.map(|v| v.to_string()).unwrap_or_default(),
        ad.today_views.map(|v| v.to_string()).unwrap_or_default(),
        excel_safe(ad.phone.as_deref().unwrap_or("")),
    ]
}

/// Собрать путь результата XLSX — тот же ключ, что и у CSV.
#[must_use]
pub fn result_path(dir: &std::path::Path, url: &str) -> std::path::PathBuf {
    super::csv::result_path(dir, url, "xlsx")
}

/// Путь единого файла результата XLSX (`avito.xlsx`).
#[must_use]
pub fn single_path(dir: &std::path::Path) -> std::path::PathBuf {
    super::csv::single_path(dir, "xlsx")
}

impl ResultExporter for XlsxExporter {
    fn save(&self, ads: &[Listing]) -> Result<()> {
        if ads.is_empty() {
            return Ok(());
        }
        self.ensure_file()?;
        self.append_rows(ads)
    }
    fn target(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_from_formula_injection() {
        assert_eq!(excel_safe("=1+1"), "'=1+1");
        assert_eq!(excel_safe("cmd"), "cmd");
        assert_eq!(excel_safe("@SUM"), "'@SUM");
    }
    #[test]
    fn time_formats_local_day() {
        // 2026-11-05 12:00:00 UTC без заданной TZ → offset 0.
        std::env::remove_var("TZ");
        let ms = 1_793_880_000_000;
        assert_eq!(ad_time(Some(ms)), "05.11.2026 12:00:00");
        assert_eq!(ad_time(None), "");
        // TZ=UTC-3 (строка parse → -180 мин по знаку lookup_offset)
        std::env::set_var("TZ", "UTC-3");
        let shifted = ad_time(Some(ms));
        assert_eq!(shifted, "05.11.2026 15:00:00");
        std::env::remove_var("TZ");
    }
    #[test]
    fn coords_render_pair() {
        let ad = Listing {
            coords: Some(serde_json::json!({"lat": 45.0, "lng": 39.0})),
            ..Default::default()
        };
        assert_eq!(coords(&ad), "45;39");
        let ad = Listing {
            coords: Some(serde_json::json!({"address_user": "Krasnodar"})),
            ..Default::default()
        };
        assert_eq!(address_user(&ad), "Krasnodar");
    }
    #[test]
    fn writes_and_rewrites_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xlsx");
        let exp = XlsxExporter::new(&path);
        let ad = Listing {
            id: "1".to_owned(),
            title: Some("T".to_owned()),
            price: Some(100),
            ..Default::default()
        };
        exp.save(std::slice::from_ref(&ad)).unwrap();
        assert!(path.exists());
    }
}
