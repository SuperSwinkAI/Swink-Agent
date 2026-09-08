//! Tests for `extract`.
#![cfg(test)]

use super::*;

#[test]
fn direct_constructor_blocks_private_ips_by_default() {
    let tool = ExtractTool::new(Arc::new(Mutex::new(None)), None, Duration::from_secs(15));

    let filter = tool
        .domain_filter
        .as_ref()
        .expect("direct extract tool should install a default filter");
    let localhost = Url::parse("http://127.0.0.1/admin").unwrap();

    assert!(filter.is_allowed(&localhost).is_err());
}

#[test]
fn content_size_bytes_sums_text_block_lengths() {
    let content = vec![
        ContentBlock::Text {
            text: "hello".to_owned(),
        },
        ContentBlock::Text {
            text: "world!".to_owned(),
        },
    ];

    assert_eq!(content_size_bytes(&content), 11);
}

#[test]
fn content_size_bytes_of_empty_content_is_zero() {
    assert_eq!(content_size_bytes(&[]), 0);
}
