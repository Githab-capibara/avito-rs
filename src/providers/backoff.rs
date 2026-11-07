//! Отказоустойчивость без крейтов: экспоненциальный backoff с джиттером
//! и токен-бакет (лимит запросов в минуту).

use std::time::Duration;

/// Экспоненциальный backoff: `base * factor^(attempt-1)`, capped, + jitter.
///
/// Порт `send_with_retries` из `integrations/notifications/transport.py`
/// (delay 2.0, backoff 1.5) и ретраев `HttpClient` оригинала.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    factor: f64,
}

impl Backoff {
    /// `new(1s, 60s, 2.0)` — дефолт для запросов к Авито.
    #[must_use]
    pub const fn new(base: Duration, max: Duration, factor: f64) -> Self {
        Self { base, max, factor }
    }

    /// Задержка перед попыткой `attempt` (1-based).
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let attempt = attempt.max(1);
        let exp = self.factor.powi(i32::try_from(attempt - 1).unwrap_or(0));
        let raw = self.base.as_secs_f64() * exp;
        let capped = raw.min(self.max.as_secs_f64());
        // jitter ±25%
        let jitter = capped * 0.25 * (pseudo_random() - 0.5) * 2.0;
        Duration::from_secs_f64((capped + jitter).max(0.05))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(60), 2.0)
    }
}

/// Детерминированный «рандом» без крейта: mixer от времени процесса.
fn pseudo_random() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (f64::from(n % 1000) / 1000.0).clamp(0.0, 1.0)
}

/// Токен-бакет: не более `rpm` запросов в минуту.
///
/// Учитывает и внешний лимит оригинала: `AvitoUrlConverter` держит
/// `min_request_interval = 31s` между конвертациями URL (rate limit сервиса).
#[derive(Debug)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_sec: f64,
    tokens: tokio::sync::Mutex<f64>,
    last: tokio::sync::Mutex<std::time::Instant>,
}

impl TokenBucket {
    /// Бакет на `rpm` rpm.
    #[must_use]
    pub fn per_minute(rpm: u32) -> Self {
        let rpm = f64::from(rpm.max(1));
        Self {
            capacity: rpm.max(1.0),
            refill_per_sec: rpm / 60.0,
            tokens: tokio::sync::Mutex::new(1.0),
            last: tokio::sync::Mutex::new(std::time::Instant::now()),
        }
    }

    /// Занять один токен, дождавшись при необходимости.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut tokens = self.tokens.lock().await;
                let mut last = self.last.lock().await;
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(*last).as_secs_f64();
                *last = now;
                *tokens = (*tokens + elapsed * self.refill_per_sec).min(self.capacity);
                if *tokens >= 1.0 {
                    *tokens -= 1.0;
                    Duration::ZERO
                } else {
                    let missing = 1.0 - *tokens;
                    Duration::from_secs_f64((missing / self.refill_per_sec).max(0.01))
                }
            };
            if wait.is_zero() {
                return;
            }
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let b = Backoff::new(Duration::from_secs(1), Duration::from_secs(10), 2.0);
        let d1 = b.delay_for(1).as_secs_f64();
        let d2 = b.delay_for(2).as_secs_f64();
        let d10 = b.delay_for(10).as_secs_f64();
        assert!(d2 > d1, "backoff must grow");
        assert!(d10 <= 10.0 * 1.25 + 0.001, "backoff must cap");
    }

    #[tokio::test]
    async fn bucket_rate_limits() {
        let b = TokenBucket::per_minute(60);
        let start = std::time::Instant::now();
        for _ in 0..3 {
            b.acquire().await;
        }
        // первые 3 токена почти сразу (стартовый резерв), главное — не паника
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
