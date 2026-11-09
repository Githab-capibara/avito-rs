# Конфиг `avito-rs`

Формат: TOML. Загрузка: `load_config` (src/main.rs) → ENV отсутствует,
значения в файле; отсутствующие ключи берут дефолты из
`AppConfig::default()` (src/dto.rs).

## Корневые ключи

| Ключ | Тип | Дефолт | Смысл |
|------|-----|--------|-------|
| `interval_sec` | u64 | 60 | Пауза между проходами цикла |
| `urls` | [string] | `[]` | Отслеживаемые URL каталогов |
| `extractor` | string | `"v1_json"` | `v1_json` или `v2_html` (см. html-fallback) |
| `fetch_mode` | string | `"spfa_url"` | `spfa_url` \| `direct_html` \| `mobile_api` |
| `api_base_override` | string | — | Подмена хоста Авито (корень `/api/9/items`, `/api/1/card/items`). В оригинале нет: нужно для e2e-прогонов против мока, не для продакшена |
| `max_concurrent` | usize | 5 | Параллельных запросов |
| `req_per_min` | u64 | 30 | Глобальная скорость |
| `timeout_sec` | u64 | 30 | HTTP-таймаут |
| `pages_per_url` | usize | 1 | Страниц на URL (включая 1-ю) |
| `pause_between_links_sec` | u64 | 0 | Пауза между URL |
| `block_threshold` | usize | 3 | Сколько блокировок подряд до ротации прокси |
| `max_retries` | u32 | 3 | Повторов на запрос |
| `retry_delay_sec` | u64 | 2 | Базовая задержка перед повтором |
| `one_shot` | bool | false | Один проход и выход |
| `parse_views` | bool | false | Тянуть просмотры |
| `parse_description` | bool | false | Тянуть описание |
| `use_own_cookies` | bool | false | Читать cookies из файла |
| `own_cookies_path` | string | `storage/own_cookies.json` | Где лежат cookies |
| `db_path` | string | `database.db` | SQLite |

> `max_concurrent` ограничивает не только параллельную обработку URL в цикле,
> но и число одновременных запросов к карточкам при обогащении
> (`parse_views` / `parse_description`).
>
> В режиме `fetch_mode = "spfa_url"` конвертер URL запускается через
> `tokio::task::spawn_blocking`, поэтому не блокирует event loop даже при
> медленном ответе конвертера.
>
> При `format = "xlsx"` дополнительно пишется JSON-сайдкар
> (`<file>.json`) с уже записанными строками — без него append в `.xlsx`
> невозможен (нет zip-инфляции без крейта), и старые строки терялись бы.
>
> Конфиг монитора хранится за `Arc<RwLock<AppConfig>>` и перечитывается в
> начале каждого цикла, поэтому `PUT /urls` через REST API сразу
> подхватывается монитором (если API и монитор в одном процессе).

## `[filters]`

| Ключ | Тип | Дефолт | Смысл |
|------|-----|--------|-------|
| `keywords_black` | [string] | `[]` | Отбрасывать объявление при совпадении |
| `keywords_white` | [string] | `[]` | Оставлять только при совпадении |
| `region` | string | `null` | Подстрока в адресе |
| `sellers_blacklist` | [string] | `[]` | Чёрный список продавцов |
| `max_price` | u64 | `null` | Верхняя граница цены |
| `min_price` | u64 | 0 | Нижняя граница |
| `max_age_sec` | u64 | 0 | Возраст объявления (0 = любой) |
| `ignore_reserved` | bool | true | Пропускать `isReserved` |
| `ignore_promotion` | bool | false | Отбрасывать продвигаемые |
| `skip_seen` | bool | true | Дедуп по паре (id, price) |

## `[notifications]`

| Ключ | Тип | Дефолт | Смысл |
|------|-----|--------|-------|
| `telegram_bot_token` | string | `null` | Токен бота |
| `telegram_chat_ids` | [i64] | `[]` | Получатели |
| `telegram_only_text` | bool | false | Без картинок |
| `telegram_proxy` | string | `null` | Прокси для TG |
| `vk_token` | string | `null` | Токен VK |
| `vk_peer_ids` | [i64] | `[]` | Получатели VK |

## `[export]`

| Ключ | Тип | Дефолт | Смысл |
|------|-----|--------|-------|
| `dir` | string | `./out` | Каталог результата |
| `format` | string | `csv` | `csv` или `xlsx` (feature xlsx) |
| `one_file_per_link` | bool | false | Отдельный файл на каждый URL |

Остальные секции — `[proxy]`, `[cookie_service]`, `[security]`,
`[observability]`.

## `serve` (feature `api`)

Адрес прослушивания задаётся флагом CLI, не конфигом:

```bash
avito-rs serve --config config.toml --bind 127.0.0.1:8080
```

Эндпоинты: `GET /status`, `GET /metrics`, `GET /urls`, `PUT /urls`,
`GET /config`, `POST /start`, `POST /stop`. Аутентификация — заголовок
`X-Api-Key` со значением `[security] api_key`; пустой `api_key` отключает
проверку (только для разработки). Цикл монитора `serve` не запускает.

## Пример

```toml
interval_sec = 120
urls = [
  "https://www.avito.ru/krasnodar/telefony",
  "https://www.avito.ru/rostov-na-donu/noutbuki",
]
fetch_mode = "spfa_url"
# только для тестов/прокси — направить запросы на свой хост:
# api_base_override = "http://127.0.0.1:18080"
db_path = "/var/lib/ap-rs/database.db"

[filters]
max_price = 50000
region = "Краснодар"
keywords_black = ["на запчасти", "ремонт"]
ignore_reserved = true
skip_seen = true

[notifications]
telegram_bot_token = "123456:AAbbCCddEEff"
telegram_chat_ids = [100500, 200700]
vk_token = ""
vk_peer_ids = []

[export]
dir = "/var/lib/ap-rs/out"
format = "csv"

[security]
api_key = ""
redact_secrets = true

[observability]
level = "info"
log_dir = "logs"
log_keep_days = 7
log_max_files = 20
```

Готовый файл со всеми ключами — `config.example.toml` в корне репозитория.

## ENV

Переменных нет: `load_config` читает только TOML. Секреты (TG/VK token,
`api_key`) Redactor вырезает из логов.

## Проверка качества

```bash
cargo build        # сборка
cargo test         # все тесты (включая e2e против мока Авито)
cargo clippy       # линтер (должен быть чист)
cargo fmt --check  # форматирование
```

## Известные ограничения (будут исправлены отдельно)

- `Listing::url()` fallback, если в объявлении нет `urlPath`, составляет
  `https://www.avito.ru/{id}` — такая страница на Авито не существует;
  экстрактор всегда заполняет `urlPath`, поэтому fallback не применяется.
- `tests/datasets.rs` читает сгенерированные файлы
  `evals/datasets/v1/*.json` — они создаются игнорируемым тестом
  `gen_datasets` (`cargo test --test gen_datasets -- --ignored`).
- API-ключ сравнивается не константное время (допустимо для данного класса
  приложения).
