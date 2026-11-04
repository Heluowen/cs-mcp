//! 领域错误：统一错误码，供 tool 层映射为结构化 JSON。
//!
//! 铁律 3：任何失败都返回结构化错误对象，绝不 panic、绝不进程退出。
//! 因此本模块只定义"可被序列化给调用方看的错误"，不定义致命错误。

use serde::Serialize;
use thiserror::Error;

/// 结构化错误码。序列化为 `SCREAMING_SNAKE_CASE`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// 上游限流。
    RateLimited,
    /// 鉴权失败（token 缺失/无效/被拒）。
    AuthError,
    /// 请求合法但确实没有数据。
    NoData,
    /// 上游故障（5xx、超时、解析失败）。
    UpstreamError,
    /// 入参非法。
    BadInput,
}

impl ErrorCode {
    /// 错误码字符串形式（与 `Serialize` 结果一致，便于日志使用）。
    #[allow(dead_code)] // 仅测试与日志使用
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::RateLimited => "RATE_LIMITED",
            ErrorCode::AuthError => "AUTH_ERROR",
            ErrorCode::NoData => "NO_DATA",
            ErrorCode::UpstreamError => "UPSTREAM_ERROR",
            ErrorCode::BadInput => "BAD_INPUT",
        }
    }
}

/// 返回给调用方的错误对象。
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// 建议重试等待秒数。仅 `RATE_LIMITED` 通常有用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, secs: u32) -> Self {
        self.retry_after = Some(secs);
        self
    }

    pub fn bad_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadInput, message)
    }

    pub fn no_data(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NoData, message)
    }
}

/// 单个 Provider 的内部错误。
///
/// 注意：S1 阶段只会构造 `NotImplemented`（Provider 尚未接入），
/// 其余变体在 S2/S4 各数据源实现时使用。
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("鉴权失败: {0}")]
    Auth(String),

    #[error("被上游限流: {detail}")]
    RateLimited {
        retry_after: Option<u32>,
        detail: String,
    },

    #[error("上游故障: {0}")]
    Upstream(String),

    #[error("没有数据: {0}")]
    NoData(String),

    #[error("尚未实现: {0}")]
    NotImplemented(String),
}

impl ProviderError {
    /// 映射为对外错误码。
    ///
    /// `NotImplemented` 对外报 `NO_DATA`（避免暴露内部实现细节），
    /// 但消息里保留可读说明，便于 S2 阶段定位。
    pub fn to_api_error(&self) -> ApiError {
        match self {
            ProviderError::Auth(d) => ApiError::new(ErrorCode::AuthError, d.clone()),
            ProviderError::RateLimited {
                retry_after,
                detail,
            } => {
                let mut e = ApiError::new(ErrorCode::RateLimited, detail.clone());
                if let Some(s) = retry_after {
                    e = e.with_retry_after(*s);
                }
                e
            }
            ProviderError::Upstream(d) => ApiError::new(ErrorCode::UpstreamError, d.clone()),
            ProviderError::NoData(d) => ApiError::new(ErrorCode::NoData, d.clone()),
            ProviderError::NotImplemented(d) => ApiError::new(ErrorCode::NoData, d.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_serializes_upper_snake() {
        let json = serde_json::to_string(&ErrorCode::RateLimited).unwrap_or_default();
        assert_eq!(json, "\"RATE_LIMITED\"");
        assert_eq!(ErrorCode::NoData.as_str(), "NO_DATA");
    }

    #[test]
    fn retry_after_is_omitted_when_none() {
        let e = ApiError::no_data("x");
        let json = serde_json::to_string(&e).unwrap_or_default();
        assert!(!json.contains("retry_after"));
    }

    #[test]
    fn provider_error_maps_to_code() {
        let e = ProviderError::Auth("bad token".into());
        assert_eq!(e.to_api_error().code, ErrorCode::AuthError);

        let e = ProviderError::RateLimited {
            retry_after: Some(30),
            detail: "slow down".into(),
        };
        let api = e.to_api_error();
        assert_eq!(api.code, ErrorCode::RateLimited);
        assert_eq!(api.retry_after, Some(30));
    }
}
