use axum::{
    Extension, Json,
    extract::{MatchedPath, Request},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use collect_metadata::{AppError, ErrorCode, diagnostics};
use serde_json::json;
use std::time::Instant;

pub(super) type ApiError = (StatusCode, Extension<ErrorCode>, Json<AppError>);

pub(super) async fn api_diagnostics(request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return next.run(request).await;
    }
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "<unmatched>".to_owned(), |path| path.as_str().to_owned());
    let method = match *request.method() {
        axum::http::Method::GET => "GET",
        axum::http::Method::POST => "POST",
        axum::http::Method::HEAD => "HEAD",
        _ => "OTHER",
    };
    let id = diagnostics::request_id();
    diagnostics::with_request(id.clone(), async move {
        let start = Instant::now();
        let mut response = next.run(request).await;
        let status = response.status();
        if (status.is_client_error() || status.is_server_error())
            && !(status == StatusCode::NOT_FOUND && route == "/api/emby/images/{id}/{image}")
        {
            let code = response
                .extensions()
                .get::<ErrorCode>()
                .copied()
                .unwrap_or_else(|| {
                    // Axum extraction and routing errors can contain submitted data. Prefer fixed messages.
                    let (code, message) = framework_error(status);
                    let safe = api_error(status, code, message).into_response();
                    *response.body_mut() = safe.into_body();
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("application/json"),
                    );
                    response.headers_mut().remove(header::CONTENT_LENGTH);
                    code
                });
            let fields = json!({"method": method, "route": route, "status": status.as_u16(),
                "code": code, "elapsed_ms": diagnostics::elapsed_ms(start)});
            if status.is_server_error() {
                diagnostics::error("web_request_failed", fields);
            } else {
                diagnostics::warn("web_request_failed", fields);
            }
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
        }
        response.headers_mut().insert(
            "x-request-id",
            header::HeaderValue::from_str(&id).expect("UUID is a valid header value"),
        );
        response
    })
    .await
}

fn framework_error(status: StatusCode) -> (ErrorCode, &'static str) {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => (
            ErrorCode::InvalidInput,
            "请求内容格式无效，请刷新页面后重试。",
        ),
        StatusCode::NOT_FOUND => (
            ErrorCode::ResourceNotFound,
            "找不到此接口，请确认 Web 页面和服务端版本一致。",
        ),
        StatusCode::METHOD_NOT_ALLOWED => (
            ErrorCode::UnsupportedMethod,
            "接口不支持此操作，请确认客户端和服务端版本一致。",
        ),
        StatusCode::PAYLOAD_TOO_LARGE => (
            ErrorCode::RequestTooLarge,
            "提交内容超过大小限制，请缩小本次范围后重试。",
        ),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (
            ErrorCode::UnsupportedMediaType,
            "接口需要 JSON 请求，请刷新页面后重试。",
        ),
        StatusCode::UNAUTHORIZED => (ErrorCode::Unauthorized, "请登录后重试。"),
        StatusCode::FORBIDDEN => (ErrorCode::AccessDenied, "当前用户没有访问权限。"),
        StatusCode::TOO_MANY_REQUESTS => (ErrorCode::RateLimited, "请求过于频繁，请稍后重试。"),
        StatusCode::SERVICE_UNAVAILABLE => (
            ErrorCode::ServiceUnavailable,
            "服务暂不可用，请稍后重试或查看应用日志。",
        ),
        _ => (ErrorCode::InternalError, "服务端处理失败，请查看应用日志。"),
    }
}

pub(super) fn api_error(status: StatusCode, code: ErrorCode, error: impl Into<String>) -> ApiError {
    (status, Extension(code), Json(AppError::new(code, error)))
}
