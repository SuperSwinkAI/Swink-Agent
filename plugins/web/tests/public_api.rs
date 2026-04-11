use swink_agent_plugin_web::{
    DomainFilter, DomainFilterError, ExtractedElement, ExtractionPreset, FetchedContent,
    SearchError, SearchProvider, SearchProviderKind, SearchResult, Viewport, WebPlugin,
    WebPluginConfig, WebPluginConfigBuilder, WebPluginError,
};

#[test]
fn root_reexports_remain_consumable() {
    let _ = std::any::type_name::<WebPlugin>();
    let _ = std::any::type_name::<WebPluginError>();
    let _ = std::any::type_name::<WebPluginConfig>();
    let _ = std::any::type_name::<WebPluginConfigBuilder>();
    let _ = std::any::type_name::<SearchProviderKind>();
    let _ = std::any::type_name::<dyn SearchProvider>();
    let _ = std::any::type_name::<SearchResult>();
    let _ = std::any::type_name::<SearchError>();
    let _ = std::any::type_name::<FetchedContent>();
    let _ = std::any::type_name::<DomainFilter>();
    let _ = std::any::type_name::<DomainFilterError>();
    let _ = std::any::type_name::<ExtractionPreset>();
    let _ = std::any::type_name::<ExtractedElement>();
    let _ = std::any::type_name::<Viewport>();
}

#[test]
fn public_builder_and_filter_are_usable_from_root() {
    let _config = WebPlugin::builder()
        .with_search_provider(SearchProviderKind::DuckDuckGo)
        .build();

    let filter = DomainFilter::default();
    let result = filter.is_allowed(&url::Url::parse("https://example.com").unwrap());
    assert!(result.is_ok() || matches!(result, Err(DomainFilterError::DnsError(_, _))));
}
