//! Tests for `playwright`.
#![cfg(test)]

use std::collections::HashSet;
use std::path::Path;
use std::process::Command as StdCommand;

use serde_json::json;

use super::{
    BRIDGE_SCRIPT, PATH_LOOKUP_PROGRAM, parse_extract_data, parse_screenshot_data,
    resolve_node_path, write_bridge_script_temp_file,
};

#[test]
fn path_lookup_program_exists_on_the_host_platform() {
    // Windows ships `where.exe`, not `which`; probing with `which` there always
    // failed and silently degraded to the bare "node" command string.
    if cfg!(windows) {
        assert_eq!(PATH_LOOKUP_PROGRAM, "where");
    } else {
        assert_eq!(PATH_LOOKUP_PROGRAM, "which");
    }
}

#[test]
fn resolve_node_path_prefers_explicit_path() {
    let explicit = Path::new("/custom/bin/node");
    assert_eq!(resolve_node_path(Some(explicit)), explicit);
}

#[test]
fn resolve_node_path_returns_a_single_line() {
    // `where` prints one line per match; a multi-line path would be unusable as
    // a program name.
    let resolved = resolve_node_path(None);
    let resolved = resolved.to_string_lossy();
    assert!(!resolved.is_empty());
    assert_eq!(resolved.lines().count(), 1);
}

#[tokio::test]
async fn writes_unique_bridge_scripts_for_concurrent_startups() {
    let handles = (0..8).map(|_| tokio::spawn(write_bridge_script_temp_file()));
    let mut paths = Vec::new();

    for handle in handles {
        let path = handle
            .await
            .expect("task should complete")
            .expect("temp script creation should succeed");
        paths.push(path);
    }

    let unique_paths: HashSet<_> = paths.iter().cloned().collect();
    assert_eq!(unique_paths.len(), paths.len());

    for path in &paths {
        let contents = tokio::fs::read_to_string(path)
            .await
            .expect("script contents should be readable");
        assert_eq!(contents, BRIDGE_SCRIPT);
        tokio::fs::remove_file(path)
            .await
            .expect("temp script cleanup should succeed");
    }
}

#[test]
fn bridge_script_blocks_special_use_hosts() {
    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
    let node_script = format!(
        r"
const bridge = require({bridge_path});

for (const host of [
  '0.0.0.0',
  '100.64.0.1',
  '198.18.0.1',
  '192.0.2.1',
  '224.0.0.1',
  '::',
  '::1',
  'fc00::1',
  'fd00::1',
  'fe80::1',
  'ff02::1',
  '2001:db8::1',
  '::ffff:10.0.0.1',
  '::ffff:127.0.0.1',
  '0:0:0:0:0:ffff:c0a8:0101',
]) {{
  if (!bridge.isBlockedPrivateHost(host)) {{
    throw new Error('private host should be blocked: ' + host);
  }}
}}
for (const host of ['93.184.216.34', '2606:4700:4700::1111', '::ffff:93.184.216.34']) {{
  if (bridge.isBlockedPrivateHost(host)) {{
    throw new Error('public host should not be blocked: ' + host);
  }}
}}
",
        bridge_path = serde_json::to_string(&bridge_path.display().to_string())
            .expect("path should serialize"),
    );

    let output = StdCommand::new(resolve_node_path(None))
        .arg("-e")
        .arg(node_script)
        .output()
        .expect("node should run bridge host assertions");

    assert!(
        output.status.success(),
        "node assertions failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_script_filters_requests_without_playwright() {
    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
    let node_script = format!(
        r"
const bridge = require({bridge_path});

void (async () => {{
async function resolvesTo(addresses) {{
  return addresses.map((address) => {{
    const family = address.includes(':') ? 6 : 4;
    return {{ address, family }};
  }});
}}

if (!bridge.isBlockedPrivateHost('127.0.0.1') || !bridge.isBlockedPrivateHost('localhost')) {{
  throw new Error('private host detection failed');
}}
if (await bridge.blockedByFilter('https://evil.com/path', {{ allowlist: [], denylist: ['evil.com'], blockPrivateIps: true }}) === null) {{
  throw new Error('denylist filter failed');
}}
if (await bridge.blockedByFilter('https://sub.evil.com/path', {{ allowlist: [], denylist: ['evil.com'], blockPrivateIps: true }}) === null) {{
  throw new Error('bare denylist domain should block subdomains');
}}
if (await bridge.blockedByFilter('https://deep.docs.example.com/path', {{ allowlist: ['*.example.com'], denylist: [], blockPrivateIps: false }}) !== null) {{
  throw new Error('wildcard allowlist should allow subdomains');
}}
if (await bridge.blockedByFilter('https://example.com/path', {{ allowlist: ['*.example.com'], denylist: [], blockPrivateIps: false }}) === null) {{
  throw new Error('wildcard allowlist should not allow the apex domain');
}}
if (await bridge.blockedByFilter('https://api.blocked.example.com/path', {{ allowlist: ['example.com'], denylist: ['*.blocked.example.com'], blockPrivateIps: false }}) === null) {{
  throw new Error('denylist wildcard should take precedence over allowlist parent domain');
}}
if (await bridge.blockedByFilter('http://127.0.0.1/admin', {{ allowlist: [], denylist: [], blockPrivateIps: true }}) === null) {{
  throw new Error('private IP filter failed');
}}
if (await bridge.blockedByFilter(
  'https://internal.example/admin',
  {{ allowlist: [], denylist: [], blockPrivateIps: true }},
  async () => resolvesTo(['10.0.0.5'])
) === null) {{
  throw new Error('resolved private subresource filter failed');
}}
if (await bridge.blockedByFilter(
  'https://public.example/',
  {{ allowlist: [], denylist: [], blockPrivateIps: true }},
  async () => resolvesTo(['93.184.216.34'])
) !== null) {{
  throw new Error('public resolved address should not be blocked');
}}
if (!String(await bridge.blockedByFilter(
  'https://unresolvable.example/',
  {{ allowlist: [], denylist: [], blockPrivateIps: true }},
  async () => {{ throw new Error('lookup failed'); }}
)).includes('DNS resolution failed')) {{
  throw new Error('DNS failures should fail closed');
}}
}})().catch((error) => {{
  console.error(error.stack || error);
  process.exit(1);
}});
",
        bridge_path = serde_json::to_string(&bridge_path.display().to_string())
            .expect("path should serialize"),
    );

    let output = StdCommand::new(resolve_node_path(None))
        .arg("-e")
        .arg(node_script)
        .output()
        .expect("node should run bridge filter assertions");

    assert!(
        output.status.success(),
        "node assertions failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_script_uses_context_routing_with_service_workers_blocked() {
    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
    let node_script = format!(
        r"
const bridge = require({bridge_path});

void (async () => {{
const options = bridge.newContextOptions({{ viewport: {{ width: 640, height: 480 }} }});
if (options.serviceWorkers !== 'block') {{
  throw new Error('service workers should be blocked for routed browser contexts');
}}
if (options.viewport.width !== 640 || options.viewport.height !== 480) {{
  throw new Error('viewport options were not preserved: ' + JSON.stringify(options));
}}
const proxiedOptions = bridge.newContextOptions(
  {{ viewport: {{ width: 800, height: 600 }} }},
  {{ server: 'http://127.0.0.1:43210' }}
);
if (proxiedOptions.proxy.server !== 'http://127.0.0.1:43210') {{
  throw new Error('proxy options were not installed: ' + JSON.stringify(proxiedOptions));
}}

let pattern = null;
let abortReason = null;
const context = {{
  async route(routePattern, handler) {{
    pattern = routePattern;
    await handler({{
      request() {{
        return {{ url() {{ return 'http://127.0.0.1/admin'; }} }};
      }},
      async abort(reason) {{ abortReason = reason; }},
      async continue() {{ throw new Error('blocked private request should not continue'); }},
    }});
  }},
}};

const blockedReason = await bridge.installNavigationFilter(
  context,
  {{ allowlist: [], denylist: [], blockPrivateIps: true }}
);
if (pattern !== '**/*') {{
  throw new Error('context route should cover all requests, got: ' + pattern);
}}
if (abortReason !== 'blockedbyclient') {{
  throw new Error('blocked request should be aborted by client, got: ' + abortReason);
}}
if (!String(blockedReason()).includes('private/internal host')) {{
  throw new Error('blocked reason was not retained: ' + blockedReason());
}}
}})().catch((error) => {{
  console.error(error.stack || error);
  process.exit(1);
}});
",
        bridge_path = serde_json::to_string(&bridge_path.display().to_string())
            .expect("path should serialize"),
    );

    let output = StdCommand::new(resolve_node_path(None))
        .arg("-e")
        .arg(node_script)
        .output()
        .expect("node should run bridge context routing assertions");

    assert!(
        output.status.success(),
        "node assertions failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_script_resolves_proxy_targets_with_single_checked_address() {
    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
    let node_script = format!(
        r"
const bridge = require({bridge_path});

void (async () => {{
async function resolvesTo(addresses) {{
  return addresses.map((address) => {{
    const family = address.includes(':') ? 6 : 4;
    return {{ address, family }};
  }});
}}

const filter = {{ allowlist: [], denylist: [], blockPrivateIps: true }};
if (!bridge.filterNeedsProxy(filter)) {{
  throw new Error('private-IP filtering should enable the browser proxy');
}}

const target = await bridge.resolveProxyTarget(
  new URL('https://public.example/path?q=1'),
  filter,
  async () => resolvesTo(['93.184.216.34'])
);
if (target.address !== '93.184.216.34' || target.host !== 'public.example' || target.port !== 443) {{
  throw new Error('unexpected proxy target: ' + JSON.stringify(target));
}}

try {{
  await bridge.resolveProxyTarget(
    new URL('https://rebind.example/'),
    filter,
    async () => resolvesTo(['93.184.216.34', '10.0.0.5'])
  );
  throw new Error('mixed public/private DNS answers should fail closed');
}} catch (error) {{
  if (!String(error.message).includes('private/internal host')) {{
    throw error;
  }}
}}

try {{
  await bridge.resolveProxyTarget(
    new URL('https://unresolvable.example/'),
    filter,
    async () => {{ throw new Error('lookup failed'); }}
  );
  throw new Error('DNS lookup failures should fail closed');
}} catch (error) {{
  if (!String(error.message).includes('DNS resolution failed')) {{
    throw error;
  }}
}}
}})().catch((error) => {{
  console.error(error.stack || error);
  process.exit(1);
}});
",
        bridge_path = serde_json::to_string(&bridge_path.display().to_string())
            .expect("path should serialize"),
    );

    let output = StdCommand::new(resolve_node_path(None))
        .arg("-e")
        .arg(node_script)
        .output()
        .expect("node should run bridge proxy assertions");

    assert!(
        output.status.success(),
        "node assertions failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_script_exports_data_only_extract_helpers() {
    assert!(!BRIDGE_SCRIPT.contains("eval("));

    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
    let node_script = format!(
        r"
const bridge = require({bridge_path});

const linksPlan = bridge.buildExtractionPlan({{ preset: 'links' }});
if (linksPlan.selector !== 'a[href]' || linksPlan.preset !== 'links') {{
  throw new Error('unexpected links plan: ' + JSON.stringify(linksPlan));
}}

const selectorPlan = bridge.buildExtractionPlan({{ selector: '.card' }});
if (selectorPlan.selector !== '.card' || selectorPlan.preset !== null) {{
  throw new Error('unexpected selector plan: ' + JSON.stringify(selectorPlan));
}}

const customElement = bridge.extractElementData(
  {{
    tagName: 'DIV',
    textContent: '  Hello world  ',
    attributes: [{{ name: 'data-id', value: '42' }}],
    getAttribute(name) {{
      return name === 'data-id' ? '42' : null;
    }},
    innerHTML: '<span>Hello world</span>',
  }},
  null
);
if (customElement.tag !== 'div' || customElement.text !== 'Hello world' || customElement.attributes['data-id'] !== '42') {{
  throw new Error('unexpected custom element: ' + JSON.stringify(customElement));
}}

const linkElement = bridge.extractElementData(
  {{
    tagName: 'A',
    textContent: ' Docs ',
    attributes: [{{ name: 'href', value: '/docs' }}],
    getAttribute(name) {{
      return name === 'href' ? '/docs' : null;
    }},
    innerHTML: 'Docs',
  }},
  'links'
);
if (linkElement.attributes.href !== '/docs' || Object.keys(linkElement.attributes).length !== 1) {{
  throw new Error('unexpected link element: ' + JSON.stringify(linkElement));
}}

const headingElement = bridge.extractElementData(
  {{
    tagName: 'H2',
    textContent: ' Section ',
    attributes: [{{ name: 'id', value: 'section' }}],
    getAttribute() {{
      return null;
    }},
    innerHTML: 'Section',
  }},
  'headings'
);
if (headingElement.tag !== 'h2' || headingElement.text !== 'Section' || Object.keys(headingElement.attributes).length !== 0) {{
  throw new Error('unexpected heading element: ' + JSON.stringify(headingElement));
}}

const tableElement = bridge.extractElementData(
  {{
    tagName: 'TABLE',
    textContent: '',
    attributes: [],
    getAttribute() {{
      return null;
    }},
    innerHTML: '<tbody><tr><td>value</td></tr></tbody>',
  }},
  'tables'
);
if (tableElement.tag !== 'table' || !tableElement.text.includes('<tbody>')) {{
  throw new Error('unexpected table element: ' + JSON.stringify(tableElement));
}}
",
        bridge_path = serde_json::to_string(&bridge_path.display().to_string())
            .expect("path should serialize"),
    );

    let output = StdCommand::new(resolve_node_path(None))
        .arg("-e")
        .arg(node_script)
        .output()
        .expect("node should run bridge helper assertions");

    assert!(
        output.status.success(),
        "node assertions failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn screenshot_data_carries_final_url() {
    let output = parse_screenshot_data(
        Some(json!({
            "image": "abc123",
            "finalUrl": "https://example.com/final",
        })),
        "https://example.com/start",
    )
    .unwrap();

    assert_eq!(output.base64, "abc123");
    assert_eq!(output.final_url, "https://example.com/final");
}

#[test]
fn extract_data_carries_final_url() {
    let output = parse_extract_data(
        Some(json!({
            "elements": [
                {
                    "tag": "a",
                    "text": "Docs",
                    "attributes": { "href": "/docs" },
                }
            ],
            "finalUrl": "https://example.com/final",
        })),
        "https://example.com/start",
    )
    .unwrap();

    assert_eq!(output.final_url, "https://example.com/final");
    assert_eq!(output.elements.len(), 1);
    assert_eq!(output.elements[0].tag, "a");
}
