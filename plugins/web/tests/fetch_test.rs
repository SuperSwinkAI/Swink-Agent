use swink_agent_plugin_web::content::{
    extract_readable_content, is_html_content_type, truncate_content,
};

#[test]
fn truncate_content_short_text_no_truncation() {
    let text = "Hello, world!";
    let (result, truncated) = truncate_content(text, 100);
    assert_eq!(result, text);
    assert!(!truncated);
}

#[test]
fn truncate_content_exact_limit_no_truncation() {
    let text = "abcde";
    let (result, truncated) = truncate_content(text, 5);
    assert_eq!(result, text);
    assert!(!truncated);
}

#[test]
fn truncate_content_long_text_is_truncated() {
    let text = "a".repeat(1000);
    let max_len = 200;
    let (result, truncated) = truncate_content(&text, max_len);

    assert!(truncated);
    // Head = 80% of 200 = 160 chars of 'a'
    assert!(result.starts_with(&"a".repeat(160)));
    // Tail = 20% of 200 = 40 chars of 'a'
    assert!(result.ends_with(&"a".repeat(40)));
    // Contains the truncation notice
    assert!(result.contains("[... content truncated"));
    assert!(result.contains("1000 chars total"));
    assert!(result.contains("showing first 160 and last 40"));
}

#[test]
fn is_html_content_type_standard_html() {
    assert!(is_html_content_type("text/html"));
    assert!(is_html_content_type("text/html; charset=utf-8"));
    // Note: Content-Type values are typically lowercased by HTTP libraries.
    // This function does a simple substring match.
}

#[test]
fn is_html_content_type_xhtml() {
    assert!(is_html_content_type("application/xhtml+xml"));
}

#[test]
fn is_html_content_type_non_html() {
    assert!(!is_html_content_type("application/json"));
    assert!(!is_html_content_type("text/plain"));
    assert!(!is_html_content_type("image/png"));
    assert!(!is_html_content_type("application/pdf"));
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
    // The title should be extracted.
    assert!(result.title.is_some());
    // The main content should be present in the extracted text.
    assert!(result.text.contains("main article content"));
}
