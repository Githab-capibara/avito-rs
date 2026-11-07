#![cfg(feature = "api")]

//! REST API управления (feature `api`) и типы статуса.

#[cfg(feature = "api")]
pub mod server;

/// Состояние монитора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RunState {
    /// Остановлен.
    Stopped,
    /// Работает.
    Running,
    /// Завершается.
    Draining,
}

impl RunState {
    /// Строка для JSON/логов.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Draining => "draining",
        }
    }
}

/// Снимок для `GET /status`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StatusReport {
    /// Состояние цикла.
    pub state: RunState,
    /// Метрики.
    pub metrics: crate::observability::MetricsSnapshot,
    /// Число URL.
    pub urls: usize,
    /// Параметры без секретов.
    pub summary: Vec<(String, String)>,
}
