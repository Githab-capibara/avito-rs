//! CSV-экспорт: порт `parser/export/excel.py` в части формирования строк.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{csv_header_line, csv_row};
use crate::dto::Listing;

/// CSV-хранилище. Создаёт файл с заголовком при первом сохранении,
/// дописывает строки дальше (как `ExcelStorage._append_rows`).
#[derive(Debug)]
pub struct CsvExporter {
    path: PathBuf,
}

impl CsvExporter {
    /// Новый экспортёр для конкретного файла.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Гарантировать наличие заголовка.
    fn ensure_header(&self) -> Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create out dir {}", parent.display()))?;
            }
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .with_context(|| format!("create {}", self.path.display()))?;
        f.write_all(format!("{}\n", csv_header_line()).as_bytes())
            .with_context(|| format!("write header to {}", self.path.display()))?;
        Ok(())
    }
}

impl super::ResultExporter for CsvExporter {
    fn save(&self, ads: &[Listing]) -> Result<()> {
        if ads.is_empty() {
            return Ok(());
        }
        self.ensure_header()?;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| format!("append to {}", self.path.display()))?;
        for ad in ads {
            f.write_all(format!("{}\n", csv_row(ad)).as_bytes())
                .with_context(|| format!("write row to {}", self.path.display()))?;
        }
        Ok(())
    }

    fn target(&self) -> String {
        self.path.display().to_string()
    }
}

/// Собрать путь результата (порт `_build_excel_path` + `PerLinkNamingStrategy`).
///
/// - `one_file_for_link = true` →
///   `<dir>/<netloc>_<md5(url)[..10]>.<ext>` (ключ хранилища на каждый URL);
/// - иначе единый `<dir>/avito.<ext>`.
#[must_use]
pub fn result_path(dir: &std::path::Path, url: &str, ext: &str) -> std::path::PathBuf {
    let netloc = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("link")
        .to_owned();
    let hash = md5_hex(url);
    dir.join(format!("{netloc}_{}.{ext}", &hash[..10]))
}

/// Путь единого файла результата (`avito.<ext>`).
#[must_use]
pub fn single_path(dir: &std::path::Path, ext: &str) -> std::path::PathBuf {
    dir.join(format!("avito.{ext}"))
}

/// MD5 в hex (порт `hashlib.md5(url.encode()).hexdigest()`), без крейта.
#[must_use]
pub fn md5_hex(input: &str) -> String {
    let msg = &mut input.as_bytes().to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut chunk_windows: Vec<[u32; 16]> = msg
        .chunks_exact(64)
        .map(|chunk| {
            let mut words = [0u32; 16];
            for (i, w) in words.iter_mut().enumerate() {
                let s = i * 4;
                *w = u32::from_le_bytes([chunk[s], chunk[s + 1], chunk[s + 2], chunk[s + 3]]);
            }
            words
        })
        .collect();
    chunk_windows.truncate(chunk_windows.len());
    for m in chunk_windows {
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f
                .wrapping_add(a)
                .wrapping_add(k[i])
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(s[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let out: [u32; 4] = [a0, b0, c0, d0];
    let mut hex = String::with_capacity(32);
    for word in out {
        for byte in word.to_le_bytes() {
            hex.push_str(&format!("{byte:02x}"));
        }
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporters::ResultExporter as _;
    use std::path::Path;

    #[test]
    fn creates_header_then_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        let exp = CsvExporter::new(&path);
        let ad = Listing {
            id: "42".to_owned(),
            title: Some("t".to_owned()),
            price: Some(9),
            ..Listing::default()
        };
        exp.save(std::slice::from_ref(&ad)).unwrap();
        exp.save(&[ad.clone(), ad]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], super::super::csv_header_line());
        assert!(lines[1].starts_with("42,"));
    }

    #[test]
    fn empty_save_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none.csv");
        CsvExporter::new(&path).save(&[]).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn result_path_uses_netloc_and_md5_prefix() {
        // PerLinkNamingStrategy: <netloc>_<md5[..10]>.<ext>
        let url = "https://www.avito.ru/moskva/telefony?cd=1";
        let p = result_path(Path::new("/tmp"), url, "csv");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(p.parent(), Some(Path::new("/tmp")));
        assert!(name.starts_with("www.avito.ru_"), "{name}");
        assert!(name.ends_with(".csv"), "{name}");
        let hash = md5_hex(url);
        assert_eq!(name, format!("www.avito.ru_{}.csv", &hash[..10]));
        // устойчивость к URL без netloc
        let p = result_path(Path::new("/tmp"), "not-a-url", "xlsx");
        assert!(p.to_string_lossy().ends_with(".xlsx"));
    }

    #[test]
    fn single_path_is_avito_ext() {
        assert_eq!(single_path(Path::new("/tmp"), "csv"), PathBuf::from("/tmp/avito.csv"));
        assert_eq!(single_path(Path::new("/tmp"), "xlsx"), PathBuf::from("/tmp/avito.xlsx"));
    }

    #[test]
    fn md5_matches_reference_vector() {
        // RFC 1321 test vectors
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex("https://www.avito.ru/moskva/telefony"),
            md5_hex("https://www.avito.ru/moskva/telefony")
        );
    }
}
