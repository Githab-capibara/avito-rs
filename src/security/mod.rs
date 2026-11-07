//! Безопасность: редактирование секретов в логах и проверка API-ключа.
//!
//! Оригинал делает это regex-костылём (`hide_private_data.py`: `***` вместо
//! значений токенов/прокси). Здесь — типизированный [`Redactor`], который
//! вызывается в каждой точке логирования, где появляется пользовательский текст.

use std::sync::Arc;

use crate::dto::AppConfig;
/// Редактор секретов в тексте.
///
/// Прямые замены без regex: для каждого секрета в конфиге — `String::replace`.
/// Так достигается O(n·k), но без лишней зависимости и без переполнения
/// backtracking'ом в крейте regex.
#[derive(Debug, Clone)]
pub struct Redactor {
    secrets: Vec<String>,
    enabled: bool,
}

impl Redactor {
    /// Построить редактор из конфига.
    #[must_use]
    pub fn from_config(cfg: &AppConfig) -> Self {
        if !cfg.security.redact_secrets {
            return Self {
                secrets: Vec::new(),
                enabled: false,
            };
        }
        let mut secrets: Vec<String> = Vec::new();
        let mut add = |v: &str| {
            let v = v.trim();
            if v.len() >= 6 {
                secrets.push(v.to_owned());
            }
        };
        if let Some(t) = &cfg.notifications.telegram_bot_token {
            add(t);
        }
        if let Some(t) = &cfg.notifications.vk_token {
            add(t);
        }
        if let Some(k) = cfg.cookie_service.api_key.as_str_opt() {
            secrets.push(k.to_owned());
        }
        if let Some(k) = cfg.security.api_key.as_str_opt() {
            secrets.push(k.to_owned());
        }
        for p in &cfg.proxy.list {
            // user:pass@host:port — маскируем и весь URL, и только креды
            if let Some(at) = p.find('@') {
                let creds = &p[..at];
                let creds = creds.trim_start_matches("http://").trim_start_matches("socks5://");
                if !creds.is_empty() {
                    secrets.push(format!("{}@", creds));
                }
            }
        }
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Self {
            secrets,
            enabled: true,
        }
    }

    /// Заменить все известные секреты на `***`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        if !self.enabled || self.secrets.is_empty() {
            return text.to_owned();
        }
        let mut out = text.to_owned();
        for s in &self.secrets {
            if s.is_empty() {
                continue;
            }
            out = out.replace(s.as_str(), "[redacted]");
        }
        out
    }

    /// Проверить API-ключ запроса (`X-Api-Key`). Пустой ключ в конфиге — проверка отключена.
    #[must_use]
    pub fn api_key_ok(&self, cfg: &AppConfig, presented: Option<&str>) -> bool {
        if cfg.security.api_key.is_empty() {
            return true;
        }
        matches!(presented, Some(p) if p == cfg.security.api_key)
    }
}

/// Трейт для выбора `Option<&str>` из строковых полей конфига.
trait AsStrOpt {
    fn as_str_opt(&self) -> Option<&str>;
}

impl AsStrOpt for String {
    fn as_str_opt(&self) -> Option<&str> {
        let v = self.trim();
        (!v.is_empty()).then_some(v)
    }
}

/// Псевдоним для совместного владения в провайдерах.
pub type SharedRedactor = Arc<Redactor>;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_secrets(tg: &str, proxy: &str) -> AppConfig {
        let mut c = AppConfig::default();
        c.notifications.telegram_bot_token = Some(tg.to_owned());
        c.proxy.list = vec![proxy.to_owned()];
        c
    }

    #[test]
    fn redacts_telegram_token() {
        let r = Redactor::from_config(&cfg_with_secrets("123456:AAbbCCddEEff", ""));
        assert_eq!(r.redact("token=123456:AAbbCCddEEff"), "token=[redacted]");
    }

    #[test]
    fn redacts_proxy_credentials() {
        let r = Redactor::from_config(&cfg_with_secrets("", "http://user:secretpass@1.2.3.4:8080"));
        let out = r.redact("via http://user:secretpass@1.2.3.4:8080 now");
        assert!(!out.contains("secretpass"), "password leaked: {out}");
    }

    #[test]
    fn short_secrets_are_ignored() {
        let r = Redactor::from_config(&cfg_with_secrets("abc", ""));
        assert_eq!(r.redact("abc stays"), "abc stays");
    }

    #[test]
    fn api_key_gate() {
        let mut c = AppConfig::default();
        c.security.api_key = "supersecretkey".to_owned();
        let r = Redactor::from_config(&c);
        assert!(r.api_key_ok(&c, Some("supersecretkey")));
        assert!(!r.api_key_ok(&c, Some("wrong")));
        assert!(!r.api_key_ok(&c, None));
        c.security.api_key = String::new();
        assert!(r.api_key_ok(&c, None), "empty key disables gate");
    }

    #[test]
    fn disabled_redactor_passes_through() {
        let mut c = cfg_with_secrets("123456:AAbbCCddEEff", "");
        c.security.redact_secrets = false;
        let r = Redactor::from_config(&c);
        assert_eq!(r.redact("123456:AAbbCCddEEff"), "123456:AAbbCCddEEff");
    }
}
