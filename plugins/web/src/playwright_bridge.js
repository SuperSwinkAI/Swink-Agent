// Playwright bridge script — communicates with Rust via JSON lines on stdin/stdout.
// Usage: node playwright_bridge.js

const net = require('net');
const dns = require('dns').promises;
const http = require('http');
const readline = require('readline');

let browser = null;
let chromium = null;

function respond(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

function resolveChromium() {
  if (!chromium) {
    ({ chromium } = require('playwright'));
  }
  return chromium;
}

async function ensureBrowser() {
  if (!browser) {
    browser = await resolveChromium().launch({ headless: true });
  }
  return browser;
}

function normalizeHost(host) {
  const rawHost = String(host || '');
  if (rawHost.startsWith('[') && rawHost.endsWith(']')) {
    return rawHost.slice(1, -1);
  }
  return rawHost;
}

function hostMatches(list, host) {
  const lowerHost = (host || '').toLowerCase();
  return (list || []).some((entry) => lowerHost === String(entry).toLowerCase());
}

function parseIpv4Bytes(host) {
  const parts = host.split('.').map((part) => Number(part));
  if (parts.length !== 4 || parts.some((part) => !Number.isInteger(part) || part < 0 || part > 255)) {
    return null;
  }
  return parts;
}

function isPrivateIpv4Bytes(parts) {
  return (
    parts[0] === 0 ||
    parts[0] === 10 ||
    parts[0] === 127 ||
    (parts[0] === 100 && parts[1] >= 64 && parts[1] <= 127) ||
    (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) ||
    (parts[0] === 192 && parts[1] === 168) ||
    (parts[0] === 169 && parts[1] === 254) ||
    (parts[0] === 198 && parts[1] >= 18 && parts[1] <= 19) ||
    (parts[0] === 192 && parts[1] === 0 && parts[2] === 2) ||
    (parts[0] === 198 && parts[1] === 51 && parts[2] === 100) ||
    (parts[0] === 203 && parts[1] === 0 && parts[2] === 113) ||
    parts[0] >= 224
  );
}

function isPrivateIpv4(host) {
  const parts = parseIpv4Bytes(host);
  return parts !== null && isPrivateIpv4Bytes(parts);
}

function parseIpv6Segments(host) {
  let input = host.toLowerCase().split('%')[0];

  if (input.includes('.')) {
    const lastColon = input.lastIndexOf(':');
    if (lastColon === -1) {
      return null;
    }
    const v4 = parseIpv4Bytes(input.slice(lastColon + 1));
    if (v4 === null) {
      return null;
    }
    input = `${input.slice(0, lastColon)}:${((v4[0] << 8) | v4[1]).toString(16)}:${((v4[2] << 8) | v4[3]).toString(16)}`;
  }

  const halves = input.split('::');
  if (halves.length > 2) {
    return null;
  }

  const left = halves[0] ? halves[0].split(':') : [];
  const right = halves.length === 2 && halves[1] ? halves[1].split(':') : [];
  const fill = halves.length === 2 ? 8 - left.length - right.length : 0;
  if (fill < 0 || (halves.length === 1 && left.length !== 8)) {
    return null;
  }

  const segments = [...left, ...Array(fill).fill('0'), ...right];
  if (segments.length !== 8) {
    return null;
  }

  return segments.map((segment) => {
    if (!/^[0-9a-f]{1,4}$/.test(segment)) {
      return null;
    }
    return Number.parseInt(segment, 16);
  });
}

function isPrivateIpv6(host) {
  const segments = parseIpv6Segments(host);
  if (segments === null || segments.some((segment) => segment === null)) {
    return false;
  }

  const mappedV4 =
    segments.slice(0, 5).every((segment) => segment === 0) && segments[5] === 0xffff;
  const compatibleV4 =
    segments.slice(0, 6).every((segment) => segment === 0) &&
    (segments[6] !== 0 || segments[7] !== 0);
  if (mappedV4 || compatibleV4) {
    return isPrivateIpv4Bytes([
      segments[6] >> 8,
      segments[6] & 0xff,
      segments[7] >> 8,
      segments[7] & 0xff,
    ]);
  }

  const unspecified = segments.every((segment) => segment === 0);
  const loopback = segments.slice(0, 7).every((segment) => segment === 0) && segments[7] === 1;
  return (
    unspecified ||
    loopback ||
    (segments[0] & 0xfe00) === 0xfc00 ||
    (segments[0] & 0xffc0) === 0xfe80 ||
    (segments[0] & 0xff00) === 0xff00 ||
    (segments[0] === 0x2001 && segments[1] === 0x0db8)
  );
}

function isBlockedPrivateHost(host) {
  const normalizedHost = normalizeHost(host);
  const lowerHost = normalizedHost.toLowerCase();
  if (lowerHost === 'localhost' || lowerHost.endsWith('.localhost')) {
    return true;
  }

  const ipVersion = net.isIP(normalizedHost);
  if (ipVersion === 4) {
    return isPrivateIpv4(normalizedHost);
  }
  if (ipVersion === 6) {
    return isPrivateIpv6(normalizedHost);
  }
  return false;
}

async function resolvesToBlockedPrivateAddress(host, lookup = dns.lookup) {
  const normalizedHost = normalizeHost(host);
  if (net.isIP(normalizedHost)) {
    return isBlockedPrivateHost(normalizedHost) ? normalizedHost : null;
  }

  const addrs = await lookup(normalizedHost, { all: true, verbatim: true });
  const results = Array.isArray(addrs) ? addrs : [addrs];
  for (const result of results) {
    const address = typeof result === 'string' ? result : result.address;
    if (address && isBlockedPrivateHost(address)) {
      return address;
    }
  }
  return null;
}

async function blockedByFilter(rawUrl, filter, lookup = dns.lookup) {
  if (!filter) {
    return null;
  }

  let parsed;
  try {
    parsed = new URL(rawUrl);
  } catch (err) {
    return `Invalid URL '${rawUrl}': ${err.message}`;
  }

  const scheme = parsed.protocol.replace(':', '');
  if (scheme !== 'http' && scheme !== 'https') {
    return `URL scheme '${scheme}' is not allowed; only http and https are permitted`;
  }

  const host = normalizeHost(parsed.hostname);
  if ((filter.allowlist || []).length > 0 && !hostMatches(filter.allowlist, host)) {
    return `Domain '${host}' is not on the allow list`;
  }
  if (hostMatches(filter.denylist, host)) {
    return `Domain '${host}' is on the deny list`;
  }
  if (filter.blockPrivateIps && isBlockedPrivateHost(host)) {
    return `Address ${host} is a private/internal host and is blocked`;
  }
  if (filter.blockPrivateIps) {
    try {
      const blockedAddress = await resolvesToBlockedPrivateAddress(host, lookup);
      if (blockedAddress) {
        return `Address ${blockedAddress} is a private/internal host and is blocked`;
      }
    } catch (err) {
      return `DNS resolution failed for '${host}': ${err.message}`;
    }
  }

  return null;
}

function newContextOptions(req, proxy) {
  const options = { serviceWorkers: 'block' };
  if (req.viewport) {
    options.viewport = { width: req.viewport.width, height: req.viewport.height };
  }
  if (proxy) {
    options.proxy = { server: proxy.server };
  }
  return options;
}

function filterNeedsProxy(filter) {
  return Boolean(filter && filter.blockPrivateIps);
}

function defaultPortForScheme(scheme) {
  return scheme === 'https' ? 443 : 80;
}

async function resolveProxyTarget(parsed, filter, lookup = dns.lookup) {
  const scheme = parsed.protocol.replace(':', '');
  if (scheme !== 'http' && scheme !== 'https') {
    throw new Error(`URL scheme '${scheme}' is not allowed; only http and https are permitted`);
  }

  const host = normalizeHost(parsed.hostname);
  if ((filter.allowlist || []).length > 0 && !hostMatches(filter.allowlist, host)) {
    throw new Error(`Domain '${host}' is not on the allow list`);
  }
  if (hostMatches(filter.denylist, host)) {
    throw new Error(`Domain '${host}' is on the deny list`);
  }
  if (filter.blockPrivateIps && isBlockedPrivateHost(host)) {
    throw new Error(`Address ${host} is a private/internal host and is blocked`);
  }

  let address = host;
  if (filter.blockPrivateIps && !net.isIP(host)) {
    let addrs;
    try {
      addrs = await lookup(host, { all: true, verbatim: true });
    } catch (err) {
      throw new Error(`DNS resolution failed for '${host}': ${err.message}`);
    }

    const results = Array.isArray(addrs) ? addrs : [addrs];
    if (results.length === 0) {
      throw new Error(`DNS resolution failed for '${host}': no addresses found`);
    }

    address = null;
    for (const result of results) {
      const resolved = typeof result === 'string' ? result : result.address;
      if (resolved && isBlockedPrivateHost(resolved)) {
        throw new Error(`Address ${resolved} is a private/internal host and is blocked`);
      }
      if (!address && resolved) {
        address = resolved;
      }
    }
    if (!address) {
      throw new Error(`DNS resolution failed for '${host}': no addresses found`);
    }
  }

  return {
    address,
    host,
    hostHeader: parsed.host,
    port: Number(parsed.port || defaultPortForScheme(scheme)),
  };
}

async function createFilteringProxy(filter) {
  const server = http.createServer(async (clientReq, clientRes) => {
    try {
      const parsed = new URL(clientReq.url);
      const target = await resolveProxyTarget(parsed, filter);
      const headers = { ...clientReq.headers, host: target.hostHeader };
      const upstream = http.request(
        {
          host: target.address,
          port: target.port,
          method: clientReq.method,
          path: `${parsed.pathname}${parsed.search}`,
          headers,
        },
        (upstreamRes) => {
          clientRes.writeHead(upstreamRes.statusCode || 502, upstreamRes.headers);
          upstreamRes.pipe(clientRes);
        }
      );
      upstream.on('error', (err) => {
        if (!clientRes.headersSent) {
          clientRes.writeHead(502);
        }
        clientRes.end(err.message);
      });
      clientReq.pipe(upstream);
    } catch (err) {
      clientRes.writeHead(403, { 'content-type': 'text/plain; charset=utf-8' });
      clientRes.end(err.message);
    }
  });

  server.on('connect', async (req, clientSocket, head) => {
    try {
      const parsed = new URL(`https://${req.url}`);
      const target = await resolveProxyTarget(parsed, filter);
      const upstream = net.connect({ host: target.address, port: target.port }, () => {
        clientSocket.write('HTTP/1.1 200 Connection Established\r\n\r\n');
        if (head && head.length > 0) {
          upstream.write(head);
        }
        upstream.pipe(clientSocket);
        clientSocket.pipe(upstream);
      });
      upstream.on('error', (err) => {
        clientSocket.end(`HTTP/1.1 502 Bad Gateway\r\n\r\n${err.message}`);
      });
      clientSocket.on('error', () => upstream.destroy());
    } catch (err) {
      clientSocket.end(`HTTP/1.1 403 Forbidden\r\n\r\n${err.message}`);
    }
  });

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });

  const address = server.address();
  return {
    server: `http://127.0.0.1:${address.port}`,
    async close() {
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

async function installNavigationFilter(context, filter) {
  let blockedReason = null;
  if (!filter) {
    return () => blockedReason;
  }

  await context.route('**/*', async (route) => {
    const reason = await blockedByFilter(route.request().url(), filter);
    if (reason) {
      blockedReason = reason;
      await route.abort('blockedbyclient');
      return;
    }

    await route.continue();
  });

  return () => blockedReason;
}

async function handleScreenshot(req) {
  const b = await ensureBrowser();
  const proxy = filterNeedsProxy(req.filter) ? await createFilteringProxy(req.filter) : null;
  let context = null;
  let blockedReason = () => null;
  try {
    context = await b.newContext(newContextOptions(req, proxy));
    blockedReason = await installNavigationFilter(context, req.filter);
    const page = await context.newPage();
    await page.goto(req.url, { waitUntil: 'load', timeout: 30000 });
    const buf = await page.screenshot({ type: 'png', fullPage: false });
    const base64 = buf.toString('base64');
    respond({ id: req.id, ok: true, data: { image: base64, finalUrl: page.url() } });
  } catch (err) {
    respond({ id: req.id, ok: false, error: blockedReason() || err.message });
  } finally {
    if (context) {
      await context.close();
    }
    if (proxy) {
      await proxy.close();
    }
  }
}

function buildExtractionPlan(req) {
  let selector = req.selector || null;

  if (req.preset) {
    switch (req.preset) {
      case 'links':
        selector = 'a[href]';
        break;
      case 'headings':
        selector = 'h1,h2,h3,h4,h5,h6';
        break;
      case 'tables':
        selector = 'table';
        break;
      default:
        return { error: `Unknown preset: ${req.preset}` };
    }
  }

  if (!selector) {
    return { error: 'No selector or preset provided' };
  }

  return { selector, preset: req.preset || null };
}

function collectAttributes(el) {
  if (!el.attributes) {
    return {};
  }

  const attributes = Array.isArray(el.attributes) ? el.attributes : Array.from(el.attributes);
  return Object.fromEntries(attributes.map((attr) => [attr.name, attr.value]));
}

function extractElementData(el, preset) {
  const tag = (el.tagName || '').toLowerCase();
  const text = (el.textContent || '').trim().slice(0, 500);

  switch (preset) {
    case 'links':
      return {
        tag,
        text,
        attributes: { href: el.getAttribute('href') || '' },
      };
    case 'headings':
      return {
        tag,
        text,
        attributes: {},
      };
    case 'tables':
      return {
        tag: 'table',
        text: (el.innerHTML || '').slice(0, 2000),
        attributes: {},
      };
    default:
      return {
        tag,
        text,
        attributes: collectAttributes(el),
      };
  }
}

async function handleExtract(req) {
  const b = await ensureBrowser();
  const proxy = filterNeedsProxy(req.filter) ? await createFilteringProxy(req.filter) : null;
  let context = null;
  let blockedReason = () => null;
  try {
    context = await b.newContext(newContextOptions(req, proxy));
    blockedReason = await installNavigationFilter(context, req.filter);
    const page = await context.newPage();
    await page.goto(req.url, { waitUntil: 'load', timeout: 30000 });

    const plan = buildExtractionPlan(req);
    if (plan.error) {
      respond({ id: req.id, ok: false, error: plan.error });
      return;
    }

    const elements = await page.evaluate(
      ({ selector, preset }) => {
        function collectAttributes(el) {
          return Object.fromEntries(Array.from(el.attributes).map((attr) => [attr.name, attr.value]));
        }

        function extractElementData(el, activePreset) {
          const tag = el.tagName.toLowerCase();
          const text = (el.textContent || '').trim().slice(0, 500);

          switch (activePreset) {
            case 'links':
              return {
                tag,
                text,
                attributes: { href: el.getAttribute('href') || '' },
              };
            case 'headings':
              return {
                tag,
                text,
                attributes: {},
              };
            case 'tables':
              return {
                tag: 'table',
                text: el.innerHTML.slice(0, 2000),
                attributes: {},
              };
            default:
              return {
                tag,
                text,
                attributes: collectAttributes(el),
              };
          }
        }

        const elements = document.querySelectorAll(selector);
        return Array.from(elements).slice(0, 200).map((el) => extractElementData(el, preset));
      },
      plan
    );

    respond({ id: req.id, ok: true, data: { elements, finalUrl: page.url() } });
  } catch (err) {
    respond({ id: req.id, ok: false, error: blockedReason() || err.message });
  } finally {
    if (context) {
      await context.close();
    }
    if (proxy) {
      await proxy.close();
    }
  }
}

async function handleRequest(line) {
  let req;
  try {
    req = JSON.parse(line);
  } catch (err) {
    respond({ id: 0, ok: false, error: `Invalid JSON: ${err.message}` });
    return;
  }

  const action = req.action;
  try {
    switch (action) {
      case 'ping':
        respond({ id: req.id, ok: true });
        break;
      case 'screenshot':
        await handleScreenshot(req);
        break;
      case 'extract':
        await handleExtract(req);
        break;
      case 'shutdown':
        respond({ id: req.id, ok: true });
        if (browser) {
          await browser.close();
          browser = null;
        }
        process.exit(0);
        break;
      default:
        respond({ id: req.id, ok: false, error: `Unknown action: ${action}` });
    }
  } catch (err) {
    respond({ id: req.id || 0, ok: false, error: err.message });
  }
}

async function handleClose() {
  if (browser) {
    await browser.close();
    browser = null;
  }
  process.exit(0);
}

if (require.main === module) {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false,
  });

  rl.on('line', (line) => {
    void handleRequest(line);
  });

  rl.on('close', () => {
    void handleClose();
  });
}

module.exports = {
  blockedByFilter,
  buildExtractionPlan,
  collectAttributes,
  createFilteringProxy,
  extractElementData,
  filterNeedsProxy,
  installNavigationFilter,
  isBlockedPrivateHost,
  newContextOptions,
  resolveProxyTarget,
};
