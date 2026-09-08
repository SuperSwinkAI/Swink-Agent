//! Tests for `screenshot`.
#![cfg(test)]

use super::*;

#[test]
fn direct_constructor_blocks_private_ips_by_default() {
    let tool = ScreenshotTool::new(
        Arc::new(Mutex::new(None)),
        None,
        Viewport {
            width: 1280,
            height: 720,
        },
        Duration::from_secs(15),
    );

    let filter = tool
        .domain_filter
        .as_ref()
        .expect("direct screenshot tool should install a default filter");
    let localhost = Url::parse("http://127.0.0.1/admin").unwrap();

    assert!(filter.is_allowed(&localhost).is_err());
}

#[test]
fn image_size_bytes_sums_base64_image_block_lengths() {
    let content = vec![ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: "image/png".into(),
            data: "AAAAAAAAAA".into(),
        },
    }];

    assert_eq!(image_size_bytes(&content), 10);
}

#[test]
fn image_size_bytes_of_empty_content_is_zero() {
    assert_eq!(image_size_bytes(&[]), 0);
}
