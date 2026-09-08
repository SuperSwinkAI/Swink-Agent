//! Tests for `azure`.
#![cfg(test)]

use super::*;

#[test]
fn azure_clouds_build_expected_token_urls() {
    assert_eq!(
        AzureCloud::Public.token_url("tenant"),
        "https://login.microsoftonline.com/tenant/oauth2/v2.0/token"
    );
    assert_eq!(
        AzureCloud::Gcc.token_url("tenant"),
        "https://login.microsoftonline.com/tenant/oauth2/v2.0/token"
    );
    assert_eq!(
        AzureCloud::GccHigh.token_url("tenant"),
        "https://login.microsoftonline.us/tenant/oauth2/v2.0/token"
    );
    assert_eq!(
        AzureCloud::Dod.token_url("tenant"),
        "https://login.microsoftonline.us/tenant/oauth2/v2.0/token"
    );
    assert_eq!(
        AzureCloud::CustomAuthorityHost("https://login.example/".to_string()).token_url("tenant"),
        "https://login.example/tenant/oauth2/v2.0/token"
    );
}
