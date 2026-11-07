//! Axum-сервер управления: `run`, `status`, `urls`, `config`, `metrics`.
//!
//! Порт одноимённой фичи оригинала из `parser_api/` (FastAPI там) — здесь тот
//! же набор эндпоинтов и заголовок `X-Api-Key`, но на axum.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use tokio::sync::{watch, RwLock};

/// Секретная строка доступа (API-ключ).
#[derive(Clone)]
pub struct ApiKey(String);

/// Состояние сервера, разделяемое с монитором.
pub struct ApiState {
    /// Живой конфиг (можно править URL через `PUT /urls`).
    cfg: RwLock<Arc<crate::dto::AppConfig>>,
    /// Метрики процесса.
    metrics: Arc<crate::observability::Metrics>,
    /// Текущее состояние.
    run_state: RwLock<crate::api::RunState>,
    /// Сигнал очереди к запуску/останову.
    signal: watch::Sender<bool>,
    /// Исходное значение сигнала (для рестарта).
    initial: watch::Receiver<bool>,
}

impl ApiState {
    /// Создать разделяемое состояние.
    #[must_use]
    pub fn new(
        cfg: Arc<crate::dto::AppConfig>,
        metrics: Arc<crate::observability::Metrics>,
    ) -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            cfg: RwLock::new(cfg),
            metrics,
            run_state: RwLock::new(crate::api::RunState::Stopped),
            signal: tx,
            initial: rx,
        }
    }

    /// Ждущий shutdown-приёмник наружу.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.initial.clone()
    }

    /// Снимок статуса.
    pub async fn status(&self) -> crate::api::StatusReport {
        let state = *self.run_state.read().await;
        let cfg = self.cfg.read().await.clone();
        crate::api::StatusReport {
            state,
            metrics: self.metrics.snapshot(),
            urls: cfg.urls.len(),
            summary: cfg.flat_for_log().into_iter().collect(),
        }
    }

    /// Сменить состояние.
    pub async fn set_state(&self, state: crate::api::RunState) {
        *self.run_state.write().await = state;
    }
}

/// Роутер API.
#[must_use]
pub fn router(state: Arc<ApiState>, api_key: String) -> Router {
    let authed = Router::new()
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/urls", get(list_urls).put(set_urls))
        .route("/config", get(get_config))
        .route("/metrics", get(get_metrics))
        .route("/status", get(get_status))
        .layer(axum::Extension(ApiKey(api_key)))
        .with_state(state);
    authed
}

/// Пустой ответ 200.
fn ok() -> Response {
    (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
}

/// Ошибка доступа.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "invalid or missing X-Api-Key"})),
    )
        .into_response()
}

/// Проверка ключа из заголовка.
fn key_ok(_state: &ApiState, headers: &HeaderMap, expected: &ApiKey) -> bool {
    if expected.0.is_empty() {
        return true;
    }
    headers
        .get(header::HeaderName::from_static("x-api-key"))
        .and_then(|v| v.to_str().ok())
        .is_some_and(|k| k == expected.0)
}

async fn start(
    State(state): State<Arc<ApiState>>,
    axum::Extension(key): axum::Extension<ApiKey>,
    headers: HeaderMap,
) -> Response {
    if !key_ok(&state, &headers, &key) {
        return unauthorized();
    }
    state.set_state(crate::api::RunState::Running).await;
    let _ = state.signal.send(false);
    ok()
}

async fn stop(
    State(state): State<Arc<ApiState>>,
    axum::Extension(key): axum::Extension<ApiKey>,
    headers: HeaderMap,
) -> Response {
    if !key_ok(&state, &headers, &key) {
        return unauthorized();
    }
    state.set_state(crate::api::RunState::Draining).await;
    let _ = state.signal.send(true);
    ok()
}

async fn get_status(State(state): State<Arc<ApiState>>) -> Response {
    Json(state.status().await).into_response()
}

async fn get_metrics(State(state): State<Arc<ApiState>>) -> Response {
    Json(state.metrics.snapshot()).into_response()
}

async fn list_urls(State(state): State<Arc<ApiState>>) -> Response {
    let cfg = state.cfg.read().await.clone();
    Json(
        cfg.urls
            .iter()
            .map(|u| u.value.clone())
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn set_urls(
    State(state): State<Arc<ApiState>>,
    axum::Extension(key): axum::Extension<ApiKey>,
    headers: HeaderMap,
    Json(urls): Json<Vec<String>>,
) -> Response {
    if !key_ok(&state, &headers, &key) {
        return unauthorized();
    }
    let current = state.cfg.read().await.clone();
    let mut next = (*current).clone();
    next.urls = urls
        .into_iter()
        .map(|value| crate::dto::TrackedUrl { value })
        .collect();
    let n = next.urls.len();
    *state.cfg.write().await = Arc::new(next);
    Json(serde_json::json!({"ok": true, "urls": n})).into_response()
}

async fn get_config(State(state): State<Arc<ApiState>>) -> Response {
    let cfg = state.cfg.read().await.clone();
    // безопасно: секреты не сериализуются наружу
    Json(cfg.flat_for_log()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_transitions() {
        let cfg = Arc::new(crate::dto::AppConfig::default());
        let st = ApiState::new(cfg, Arc::new(crate::observability::Metrics::new()));
        assert_eq!(st.status().await.state, crate::api::RunState::Stopped);
        st.set_state(crate::api::RunState::Running).await;
        assert_eq!(st.status().await.state, crate::api::RunState::Running);
    }

    #[test]
    fn empty_key_disables_gate() {
        let mut headers = HeaderMap::new();
        let st = ApiState::new(
            Arc::new(crate::dto::AppConfig::default()),
            Arc::new(crate::observability::Metrics::new()),
        );
        assert!(key_ok(&st, &headers, &ApiKey(String::new())));
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_static("nope"),
        );
        assert!(!key_ok(&st, &headers, &ApiKey("secret".to_owned())));
        assert!(key_ok(&st, &headers, &ApiKey(String::new())));
    }

    #[test]
    fn matching_key_passes() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_static("secret"),
        );
        let st = ApiState::new(
            Arc::new(crate::dto::AppConfig::default()),
            Arc::new(crate::observability::Metrics::new()),
        );
        assert!(key_ok(&st, &headers, &ApiKey("secret".to_owned())));
    }
}
