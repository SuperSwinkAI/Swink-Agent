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
mod tests {
    use super::{HTTP_BODY_TRUNCATION_LIMIT, truncate_http_body};

    #[test]
    fn short_body_is_preserved() {
        assert_eq!("body", truncate_http_body("body"));
    }

    #[test]
    fn long_body_is_truncated_with_marker() {
        let body = "a".repeat(HTTP_BODY_TRUNCATION_LIMIT + 1);

        let truncated = truncate_http_body(&body);

        assert_eq!(
            format!("{}…", "a".repeat(HTTP_BODY_TRUNCATION_LIMIT)),
            truncated
        );
    }

    #[test]
    fn truncation_keeps_utf8_boundaries() {
        let mut body = "a".repeat(HTTP_BODY_TRUNCATION_LIMIT - 1);
        body.push('é');

        let truncated = truncate_http_body(&body);

        assert_eq!(
            format!("{}…", "a".repeat(HTTP_BODY_TRUNCATION_LIMIT - 1)),
            truncated
        );
    }
}
