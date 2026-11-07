//! Telegram-провайдер (`sendMessage` Bot API) без крейта `teloxide`.
//!
//! Порт `integrations/notifications/telegram.py` + `dto/telegram.py`:
//! несколько `chat_id`, опция «только текст» (без картинки).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use super::backoff::Backoff;
use crate::dto::{AppConfig, Notification};
use crate::security::Redactor;

/// Базовый URL Bot API.
const API_BASE: &str = "https://api.telegram.org";

/// Один экземпляр Telegram-бота.
#[derive(Debug)]
pub struct TelegramNotifier {
    http: reqwest::Client,
    token: String,
    chat_ids: Vec<i64>,
    /// Отправлять без `parse_mode` (HTML ломается на длинных описаниях).
    #[allow(dead_code)]
    only_text: bool,
    #[allow(dead_code)]
    proxy: Option<String>,
    redactor: Arc<Redactor>,
    lock: Mutex<()>,
}

impl TelegramNotifier {
    /// Собрать нотифаер из конфига. `None` — токен не задан.
    pub fn from_config(cfg: &AppConfig, redactor: Arc<Redactor>) -> Result<Option<Self>> {
        let Some(token) = cfg
            .notifications
            .telegram_bot_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            return Ok(None);
        };
        if cfg.notifications.telegram_chat_ids.is_empty() {
            return Ok(None);
        }
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10));
        if let Some(p) = &cfg.notifications.telegram_proxy {
            builder = builder.proxy(reqwest::Proxy::all(p).context("bad telegram proxy")?);
        }
        let http = builder.build().context("build telegram client")?;
        Ok(Some(Self {
            http,
            token: token.to_owned(),
            chat_ids: cfg.notifications.telegram_chat_ids.clone(),
            only_text: cfg.notifications.telegram_only_text,
            proxy: cfg.notifications.telegram_proxy.clone(),
            redactor,
            lock: Mutex::new(()),
        }))
    }

    /// Собрать текст сообщения из уведомления (MarkdownV2, порт `Notifier.format`).
    #[must_use]
    pub fn compose(&self, n: &Notification) -> String {
        let mut text = Self::escape_markdown_v2(&n.body);
        if n.promoted {
            text.push_str(" 🔢");
        }
        text
    }

    /// Отправить готовый текст во все чаты (порт `notify_many` для текста).
    pub async fn send_text(&self, text: &str) -> Result<()> {
        let mut first_err: Option<anyhow::Error> = None;
        for chat in &self.chat_ids {
            if let Err(e) = self.send(*chat, text).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }

    /// Порт `escape_markdown_v2`: экранировать спецсимволы разметки.
    #[must_use]
    pub fn escape_markdown_v2(text: &str) -> String {
        const SPECIAL: [char; 18] = [
            '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.',
            '!',
        ];
        let mut out = String::with_capacity(text.len() + 8);
        for c in text.chars() {
            if SPECIAL.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    /// Порт `_send_photo_bytes`: скачать картинку и отправить multipart'ом.
    async fn send_photo_bytes(&self, chat_id: i64, image_url: &str, text: &str) -> Result<()> {
        let image = self
            .http
            .get(image_url)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .with_context(|| "telegram image download")?
            .error_for_status()
            .with_context(|| "telegram image status")?
            .bytes()
            .await
            .with_context(|| "telegram image bytes")?;
        let url = format!("{API_BASE}/bot{}/sendPhoto", self.token);
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .text("caption", text.to_owned())
            .text("parse_mode", "MarkdownV2".to_owned())
            .part(
                "photo",
                reqwest::multipart::Part::bytes(image.to_vec())
                    .file_name("photo.jpg")
                    .mime_str("image/jpeg")
                    .context("photo mime")?,
            );
        let response = self.http.post(&url).multipart(form).send().await;
        match response {
            Ok(r) if r.status().is_success() => Ok(()),
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!("tg sendPhoto {status}: {}", self.redactor.redact(&body));
                anyhow::bail!("telegram sendPhoto http {status}")
            }
            Err(e) => {
                let msg = self.redactor.redact(&e.to_string());
                tracing::warn!("tg sendPhoto failed: {msg}");
                Err(anyhow::anyhow!("telegram sendPhoto failed: {e}"))
            }
        }
    }

    /// Порт `notify_ad`: текст, фото по URL, фолбэк на bytes и на text-only
    /// при `400` на sendPhoto.
    pub async fn notify_ad(&self, chat_id: i64, n: &Notification) -> Result<()> {
        let text = Self::escape_markdown_v2(&self.compose(n));
        if !self.only_text {
            if let Some(url) = &n.image_url {
                let direct = format!("{API_BASE}/bot{}/sendPhoto", self.token);
                let body = serde_json::json!({
                    "chat_id": chat_id,
                    "photo": url,
                    "caption": text,
                    "parse_mode": "MarkdownV2"
                });
                let res = self.http.post(&direct).json(&body).send().await;
                match res {
                    Ok(r) if r.status().is_success() => return Ok(()),
                    Ok(r) => {
                        let status = r.status();
                        let body = r.text().await.unwrap_or_default();
                        tracing::warn!(
                            "tg sendPhoto URL {status}: {}",
                            self.redactor.redact(&body)
                        );
                        if status.as_u16() == 400 {
                            tracing::info!(
                                "sendPhoto URL rejected (400), retrying with image bytes"
                            );
                            return match self.send_photo_bytes(chat_id, url, &text).await {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    tracing::warn!(
                                        "multipart upload failed, falling back to text-only: {e}"
                                    );
                                    self.send(chat_id, &text).await
                                }
                            };
                        }
                    }
                    Err(e) => tracing::warn!("tg sendPhoto request failed: {e}"),
                }
            }
        }
        self.send(chat_id, &text).await
    }

    /// Отправить сообщение в один чат.
    pub async fn send(&self, chat_id: i64, text: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let url = format!("{API_BASE}/bot{}/sendMessage", self.token);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "MarkdownV2",
            "disable_web_page_preview": true,
        });
        let backoff = Backoff::default();
        for attempt in 1..=3 {
            let res = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .with_context(|| {
                    format!(
                        "tg sendMessage to {chat_id}: {}",
                        self.redactor.redact(&url)
                    )
                });
            match res {
                Ok(r) if r.status().is_success() => {
                    let payload: serde_json::Value = r
                        .json()
                        .await
                        .map_err(|e| anyhow::anyhow!("tg body: {e}"))?;
                    if payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                        return Ok(());
                    }
                    tracing::warn!("tg not ok: {}", self.redactor.redact(&payload.to_string()));
                }
                Ok(r) => {
                    let status = r.status().as_u16();
                    let body_text = r.text().await.unwrap_or_default();
                    tracing::warn!("tg {status}: {}", self.redactor.redact(&body_text));
                }
                Err(e) => {
                    tracing::warn!(
                        "tg request failed: {}",
                        self.redactor.redact(&e.to_string())
                    );
                }
            }
            if attempt < 3 {
                tokio::time::sleep(backoff.delay_for(attempt)).await;
            }
        }
        Err(anyhow::anyhow!(
            "telegram sendMessage failed for chat {chat_id}"
        ))
    }

    /// Разослать уведомление во все чаты (порт `notify_many`).
    pub async fn notify(&self, n: &Notification) -> Result<()> {
        let mut first_err: Option<anyhow::Error> = None;
        for chat in &self.chat_ids {
            if let Err(e) = self.notify_ad(*chat, n).await {
                tracing::warn!(
                    "notify chat {chat} failed: {}",
                    self.redactor.redact(&e.to_string())
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }
}

/// Экранирование HTML для `parse_mode=HTML` (порт `html.escape` в оригинале).
#[must_use]
pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(html_escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }
}
