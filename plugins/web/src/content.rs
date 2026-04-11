use std::io::Cursor;

/// Errors that can occur during content extraction.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("Readability extraction failed: {0}")]
    ExtractionFailed(String),
}

/// Result of fetching and extracting content from a web page.
#[derive(Debug, Clone)]
pub struct FetchedContent {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    pub content_type: String,
    pub content_length: usize,
    pub truncated: bool,
    pub status_code: u16,
}

/// Extract readable content from raw HTML bytes using the readability algorithm.
///
/// Strips navigation, ads, scripts, and boilerplate, returning the main article
/// text with an optional title.
pub fn extract_readable_content(
    html: &[u8],
    url: &url::Url,
) -> Result<FetchedContent, ContentError> {
    let mut cursor = Cursor::new(html);
    let product = readability::extractor::extract(&mut cursor, url)
        .map_err(|e| ContentError::ExtractionFailed(e.to_string()))?;

    let title = if product.title.is_empty() {
        None
    } else {
        Some(product.title)
    };

    Ok(FetchedContent {
        url: url.to_string(),
        title,
        text: product.text,
        content_type: "text/html".to_string(),
        content_length: html.len(),
        truncated: false,
        status_code: 200,
    })
}

/// Truncate content to fit within `max_len` characters.
///
/// If the text is already within the limit, returns it unchanged with `false`.
/// Otherwise, keeps 80% from the beginning and 20% from the end, inserting a
/// truncation notice in the middle.
pub fn truncate_content(text: &str, max_len: usize) -> (String, bool) {
    if text.len() <= max_len {
        return (text.to_string(), false);
    }

    let original_len = text.len();
    let head_len = max_len * 80 / 100;
    let tail_len = max_len * 20 / 100;

    let head = &text[..head_len];
    let tail = &text[original_len - tail_len..];

    let notice = format!(
        "\n\n[... content truncated ({original_len} chars total, \
         showing first {head_len} and last {tail_len}) ...]\n\n"
    );

    let mut result = String::with_capacity(head_len + notice.len() + tail_len);
    result.push_str(head);
    result.push_str(&notice);
    result.push_str(tail);

    (result, true)
}

/// Check whether a Content-Type header value indicates HTML content.
pub fn is_html_content_type(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml")
}

#[cfg(test)]
mod tests {
    use super::{extract_readable_content, is_html_content_type, truncate_content};

    #[test]
    fn truncate_content_short_text_no_truncation() {
        let text = "Hello, world!";
        let (result, truncated) = truncate_content(text, 100);
        assert_eq!(result, text);
        assert!(!truncated);
    }

    #[test]
    fn truncate_content_long_text_is_truncated() {
        let text = "a".repeat(1000);
        let (result, truncated) = truncate_content(&text, 200);

        assert!(truncated);
        assert!(result.starts_with(&"a".repeat(160)));
        assert!(result.ends_with(&"a".repeat(40)));
        assert!(result.contains("[... content truncated"));
    }

    #[test]
    fn html_content_type_detection_matches_expected_values() {
        assert!(is_html_content_type("text/html"));
        assert!(is_html_content_type("text/html; charset=utf-8"));
        assert!(is_html_content_type("application/xhtml+xml"));
        assert!(!is_html_content_type("application/json"));
        assert!(!is_html_content_type("text/plain"));
    }

    #[test]
    fn extract_readable_content_simple_article() {
        let html = br#"
        <!DOCTYPE html>
        <html>
        <head><title>Test Article</title></head>
        <body>
            <nav>Navigation links here</nav>
            <article>
                <h1>Main Heading</h1>
                <p>This is the main article content that should be extracted by the readability algorithm. It contains enough text to be considered the primary content of the page.</p>
                <p>Here is a second paragraph with more meaningful content to help the readability algorithm identify this as the main content block of the page.</p>
            </article>
            <footer>Footer content here</footer>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/article").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        assert_eq!(result.url, "https://example.com/article");
        assert_eq!(result.content_type, "text/html");
        assert_eq!(result.content_length, html.len());
        assert!(!result.truncated);
        assert_eq!(result.status_code, 200);
        assert!(result.title.is_some());
        assert!(result.text.contains("main article content"));
    }
}
