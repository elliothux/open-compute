//! open-compute vendor extensions backed by existing domain authorities.

use crate::http::HttpState;
use axum::Router;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
}
