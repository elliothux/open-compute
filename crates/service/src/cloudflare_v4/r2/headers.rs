use super::*;

pub(super) fn header_text(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, V4Error> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    value
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| V4Error::InvalidRequest)
        })
        .transpose()
}
