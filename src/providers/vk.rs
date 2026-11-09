//! VK-провайдер: `messages.send` + загрузка фото (`wall/messages` Upload).
//!
//! Порт `integrations/notifications/vk.py` + `utils/send_report_to_vk.py`
//! (m-вызовы: `upload` → `photo.saveMessagesPhoto` → `messages.send`).
//! Без крейта `vk-sdk`, только reqwest.

use std::time::Duration;

use anyhow::{Context, Result};

use super::backoff::Backoff;
use crate::dto::{AppConfig, Notification};
use crate::security::Redactor;

/// Один экземпляр VK-бота.
#[derive(Debug)]
pub struct VkNotifier {
    http: reqwest::Client,
    token: String,
    peer_ids: Vec<i64>,
    redactor: Redactor,
    api_base: String,
}

impl VkNotifier {
    /// Собрать нотифаер. `None` — токен или получатели не заданы.
    pub fn from_config(cfg: &AppConfig, redactor: Redactor) -> Result<Option<Self>> {
        let Some(token) = cfg
            .notifications
            .vk_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            return Ok(None);
        };
        if cfg.notifications.vk_peer_ids.is_empty() {
            return Ok(None);
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build vk client")?;
        Ok(Some(Self {
            http,
            token: token.to_owned(),
            peer_ids: cfg.notifications.vk_peer_ids.clone(),
            redactor,
            api_base: "https://api.vk.com/method".to_owned(),
        }))
    }

    /// Вызов метода VK API.
    async fn call(&self, method: &str, params: &[(&str, String)]) -> Result<serde_json::Value> {
        let url = format!("{}/{method}", self.api_base);
        let backoff = Backoff::default();
        for attempt in 1..=3 {
            let mut body = params.to_vec();
            body.push(("access_token", self.token.clone()));
            body.push(("v", "5.199".to_owned()));
            let res = self.http.post(&url).form(&body).send().await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let payload: serde_json::Value = r
                        .json()
                        .await
                        .map_err(|e| anyhow::anyhow!("vk body: {e}"))?;
                    if let Some(err) = payload.get("error") {
                        let red = self.redactor.redact(&err.to_string());
                        tracing::warn!("vk {method} error: {red}");
                        if attempt == 3 {
                            return Err(anyhow::anyhow!("vk {method} failed: {red}"));
                        }
                    } else {
                        return Ok(payload);
                    }
                }
                Ok(r) => {
                    let status = r.status().as_u16();
                    let text = r.text().await.unwrap_or_default();
                    tracing::warn!("vk {method} {status}: {}", self.redactor.redact(&text));
                    if attempt == 3 {
                        return Err(anyhow::anyhow!("vk {method} http {status}"));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "vk {method} request failed: {}",
                        self.redactor.redact(&e.to_string())
                    );
                    if attempt == 3 {
                        return Err(anyhow::anyhow!("vk {method} request: {e}"));
                    }
                }
            }
            tokio::time::sleep(backoff.delay_for(attempt)).await;
        }
        Err(anyhow::anyhow!("vk {method} exhausted retries"))
    }

    /// Отправить сообщение в один peer.
    async fn send_to_peer(&self, peer: i64, n: &Notification) -> Result<()> {
        let attachment = match &n.image_url {
            Some(url) => match self.upload_photo(url).await {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        "vk photo upload failed for peer {peer}, sending text-only: {e}"
                    );
                    None
                }
            },
            None => None,
        };
        let mut params: Vec<(&str, String)> = vec![
            ("peer_id", peer.to_string()),
            ("message", n.body.clone()),
            ("random_id", "0".to_owned()),
        ];
        if let Some(a) = attachment {
            params.push(("attachment", a));
        }
        self.call("messages.send", &params).await?;
        Ok(())
    }

    /// Загрузить фото и вернуть идентификатор вложения (best-effort).
    async fn upload_photo(&self, url: &str) -> Result<Option<String>> {
        let response = self.http.get(url).send().await;
        let bytes = match response {
            Ok(r) => match r.error_for_status() {
                Ok(ok_r) => ok_r.bytes().await.ok(),
                Err(e) => {
                    tracing::warn!("vk photo download failed: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("vk photo download failed: {e}");
                None
            }
        };
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let peer = self.peer_ids.first().copied().unwrap_or(0).to_string();
        let server: serde_json::Value = self
            .call("photos.getMessagesUploadServer", &[("peer_id", peer)])
            .await?;
        let upload_url = server
            .pointer("/response/upload_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_owned();
        if upload_url.is_empty() {
            return Ok(None);
        }
        let part = reqwest::multipart::Part::bytes(bytes.to_vec());
        let form = reqwest::multipart::Form::new().part("photo", part);
        let saved = match self.http.post(&upload_url).multipart(form).send().await {
            Ok(r) => match r.error_for_status() {
                Ok(ok_r) => ok_r.json::<serde_json::Value>().await.ok(),
                Err(e) => {
                    tracing::warn!("vk photo upload failed: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("vk photo upload failed: {e}");
                None
            }
        };
        let Some(saved) = saved else {
            return Ok(None);
        };
        let server_id = saved
            .get("server")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let photo = saved
            .get("photo")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let hash = saved
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        if server_id.is_empty() || photo.is_empty() {
            return Ok(None);
        }
        let params: Vec<(&str, String)> =
            vec![("server", server_id), ("photo", photo), ("hash", hash)];
        let resp = self.call("photos.saveMessagesPhoto", &params).await?;
        // Вложение: `photo<owner>_<id>` из ответа saveMessagesPhoto
        let attachment = resp
            .pointer("/response")
            .and_then(|r| {
                let owner = r.get("owner_id").and_then(|v| v.as_str());
                let photo_id = r.get("photo_id").and_then(|v| v.as_str());
                match (owner, photo_id) {
                    (Some(o), Some(p)) => Some(format!("photo{o}_{p}")),
                    _ => None,
                }
            })
            .or_else(|| {
                resp.pointer("/response/owner_photo_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            });
        Ok(attachment)
    }

    /// Отправить готовый текст во все peer (для монитора).
    pub async fn send_text(&self, text: &str) -> Result<()> {
        let note = Notification {
            title: "Avito".to_owned(),
            body: text.to_owned(),
            link: String::new(),
            image_url: None,
            promoted: false,
        };
        self.notify(&note).await
    }

    /// Разослать уведомление (порт `notify_many`).
    pub async fn notify(&self, n: &Notification) -> Result<()> {
        let mut first_err = None;
        for peer in &self.peer_ids {
            if let Err(e) = self.send_to_peer(*peer, n).await {
                tracing::warn!("vk notify {peer} failed: {e}");
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }
}
