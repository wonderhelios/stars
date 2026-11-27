//! Embedded modular static frontend assets (single-binary zero-FS deployment).

use axum::{http::header, response::IntoResponse};

const INDEX_HTML: &str = include_str!("../../static/index.html");
const CSS_TOKENS: &str = include_str!("../../static/css/tokens.css");
const CSS_BASE: &str = include_str!("../../static/css/base.css");
const CSS_COMPONENTS: &str = include_str!("../../static/css/components.css");
const JS_MAIN: &str = include_str!("../../static/js/main.js");
const JS_CORE_API: &str = include_str!("../../static/js/core/api.js");
const JS_CORE_FMT: &str = include_str!("../../static/js/core/fmt.js");
const JS_CORE_CHART: &str = include_str!("../../static/js/core/chart.js");
const JS_CORE_ROUTER: &str = include_str!("../../static/js/core/router.js");
const JS_VIEW_OVERVIEW: &str = include_str!("../../static/js/views/overview.js");
const JS_VIEW_LIVE: &str = include_str!("../../static/js/views/live.js");
const JS_VIEW_LIMITOCO: &str = include_str!("../../static/js/views/limitoco.js");
const JS_VIEW_ARCHIVE: &str = include_str!("../../static/js/views/archive.js");

fn static_asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        ],
        body,
    )
}

pub(super) async fn index_html() -> impl IntoResponse { static_asset("text/html; charset=utf-8", INDEX_HTML) }
pub(super) async fn css_tokens() -> impl IntoResponse { static_asset("text/css; charset=utf-8", CSS_TOKENS) }
pub(super) async fn css_base() -> impl IntoResponse { static_asset("text/css; charset=utf-8", CSS_BASE) }
pub(super) async fn css_components() -> impl IntoResponse { static_asset("text/css; charset=utf-8", CSS_COMPONENTS) }
pub(super) async fn js_main() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_MAIN) }
pub(super) async fn js_core_api() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_CORE_API) }
pub(super) async fn js_core_fmt() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_CORE_FMT) }
pub(super) async fn js_core_chart() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_CORE_CHART) }
pub(super) async fn js_core_router() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_CORE_ROUTER) }
pub(super) async fn js_view_overview() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_VIEW_OVERVIEW) }
pub(super) async fn js_view_live() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_VIEW_LIVE) }
pub(super) async fn js_view_limitoco() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_VIEW_LIMITOCO) }
pub(super) async fn js_view_archive() -> impl IntoResponse { static_asset("text/javascript; charset=utf-8", JS_VIEW_ARCHIVE) }

