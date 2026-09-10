use super::super::{V4RequestContext, V4ResultInfo, paginated_response, result_info_response};
use super::invalid_response;
use axum::response::Response;
use base64::Engine as _;
use serde::Serialize;

pub(super) fn offset_page<T: Serialize>(
    context: V4RequestContext,
    result: Vec<T>,
    per_page: Option<usize>,
    page: Option<usize>,
) -> Response {
    let per_page = per_page.unwrap_or(30);
    let page = page.unwrap_or(1);
    if !(1..=100).contains(&per_page) || page == 0 {
        return invalid_response(context.request_id());
    }
    let total_count = result.len();
    let total_pages = total_count.div_ceil(per_page);
    let start = match (page - 1).checked_mul(per_page) {
        Some(value) => value.min(total_count),
        None => return invalid_response(context.request_id()),
    };
    let result = result
        .into_iter()
        .skip(start)
        .take(per_page)
        .collect::<Vec<_>>();
    let count = result.len();
    paginated_response(
        context,
        result,
        V4ResultInfo {
            page,
            per_page,
            count,
            total_count,
            total_pages,
        },
    )
}

#[derive(Serialize)]
struct CursorResultInfo {
    cursor: String,
    per_page: usize,
    count: usize,
}

pub(super) fn cursor_page<T: Serialize>(
    context: V4RequestContext,
    result: Vec<T>,
    limit: Option<usize>,
    cursor: Option<&str>,
    page: Option<usize>,
) -> Response {
    let limit = limit.unwrap_or(50);
    if !(1..=200).contains(&limit) || (cursor.is_some() && page.is_some()) {
        return invalid_response(context.request_id());
    }
    if let Some(page) = page {
        return offset_page(context, result, Some(limit), Some(page));
    }
    let start = match cursor.map(decode_cursor).transpose() {
        Ok(value) => value.unwrap_or(0),
        Err(()) => return invalid_response(context.request_id()),
    };
    if start > result.len() {
        return invalid_response(context.request_id());
    }
    let total = result.len();
    let page = result
        .into_iter()
        .skip(start)
        .take(limit)
        .collect::<Vec<_>>();
    let next = start.saturating_add(page.len());
    result_info_response(
        context,
        page,
        CursorResultInfo {
            cursor: if next < total {
                encode_cursor(next)
            } else {
                String::new()
            },
            per_page: limit,
            count: next.saturating_sub(start),
        },
    )
}

fn encode_cursor(offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(offset.to_string())
}

fn decode_cursor(value: &str) -> Result<usize, ()> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ())?;
    std::str::from_utf8(&decoded)
        .map_err(|_| ())?
        .parse()
        .map_err(|_| ())
}
