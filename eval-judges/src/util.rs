pub(crate) const HTTP_BODY_TRUNCATION_LIMIT: usize = 512;

pub(crate) fn truncate_http_body(body: &str) -> String {
    if body.len() <= HTTP_BODY_TRUNCATION_LIMIT {
        return body.to_string();
    }

    let truncate_at = body
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|&index| index <= HTTP_BODY_TRUNCATION_LIMIT)
        .last()
        .unwrap_or(0);

    format!("{}…", &body[..truncate_at])
}

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
