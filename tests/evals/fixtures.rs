//! Фикстуры golden-тестов: валидный ответ mobile API (`spfa`) →
//! предсказуемый набор объявлений.

use anyhow::Result;
use avito_rs::providers::FetchedPage;

/// Одно объявление (продуктовая форма mobile API).
#[allow(clippy::too_many_arguments)]
fn item(
    id: &str,
    url_path: &str,
    title: &str,
    desc: &str,
    price: u64,
    seller: &str,
    address: &str,
    image: &str,
    reserved: bool,
    promoted: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "urlPath": url_path,
        "title": title,
        "description": desc,
        "priceDetailed": { "value": price, "currency": "RUB" },
        "normalizedPrice": format!("{price} ₽"),
        "sortTimeStamp": 1_730_000_000_000i64,
        "seller": { "sellerId": seller, "formattedAddress": address },
        "geo": { "coordinates": { "latitude": 45.0, "longitude": 39.0 },
                 "formattedAddress": address },
        "iva": if promoted {
            serde_json::json!({ "businessUserId": 4242 })
        } else {
            serde_json::Value::Null
        },
        "images": [ { "140x105": image, "636x480": image } ],
        "isReserved": reserved,
        "totalViews": 12,
        "todayViews": 3,
    })
}

/// Товар 1: 85 000 ₽ (фильтр `max_price=50000` его отбросит).
pub const ITEM_EXPENSIVE: (&str, &str, &str, &str, u64, &str, &str, bool, bool) = (
    "4040404040",
    "/telefony/apple_iphone_16_pro_999",
    "Apple iPhone 16 Pro 999",
    "Отличное состояние, полный комплект.",
    85_000,
    "shop_target",
    "Краснодар, ул. Красная",
    false,
    false,
);

/// Товар 2: 49 999 ₽ (проходит фильтр).
pub const ITEM_CHEAP: (&str, &str, &str, &str, u64, &str, &str, bool, bool) = (
    "5050505050",
    "/telefony/samsung_galaxy_s25_ultra",
    "Samsung Galaxy S25 Ultra",
    "Новый, гарантия.",
    49_999,
    "shop_other",
    "Москва",
    true,
    true,
);

/// Ожидания (id, price, title) для golden-ассертов.
pub const EXPECTED: &[(&str, Option<u64>, &str)] = &[
    (ITEM_EXPENSIVE.0, Some(85_000), ITEM_EXPENSIVE.2),
    (ITEM_CHEAP.0, Some(49_999), ITEM_CHEAP.2),
];

/// Ответ API с двумя товарами.
pub fn raw_page() -> String {
    let items = serde_json::json!({
        "items": [item_from(ITEM_EXPENSIVE), item_from(ITEM_CHEAP)],
    });
    serde_json::to_string(&items).unwrap()
}

/// Ответ API только с дешёвым товаром (цена упала → «новое» объявление).
pub fn raw_page_cheap_only() -> String {
    let items = serde_json::json!({
        "items": [item_from(ITEM_CHEAP)],
    });
    serde_json::to_string(&items).unwrap()
}

/// Собрать JSON товара из кортежа.
fn item_from(
    (id, path, title, desc, price, seller, address, reserved, promoted): (
        &str,
        &str,
        &str,
        &str,
        u64,
        &str,
        &str,
        bool,
        bool,
    ),
) -> serde_json::Value {
    item(
        id,
        path,
        title,
        desc,
        price,
        seller,
        address,
        "https://img.test/x.jpg",
        reserved,
        promoted,
    )
}

/// Подставной фетчер: URL решает, какую фикстуру отдать.
pub struct FixtureFetcher;

impl FixtureFetcher {
    /// Конструктор.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Вернуть страницу (200 + JSON) для URL.
    pub async fn fetch(&self, url: &str) -> Result<FetchedPage> {
        let raw = if url.contains("price_drop=1") {
            raw_page_cheap_only()
        } else {
            raw_page()
        };
        Ok(FetchedPage {
            url: url.to_owned(),
            status: 200,
            body: sonic_rs::from_str(&raw).ok(),
            raw_body: raw.into_bytes(),
        })
    }
}

impl Default for FixtureFetcher {
    fn default() -> Self {
        Self::new()
    }
}
