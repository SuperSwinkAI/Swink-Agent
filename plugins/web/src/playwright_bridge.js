// Playwright bridge script — communicates with Rust via JSON lines on stdin/stdout.
// Usage: node playwright_bridge.js

const readline = require('readline');
const { chromium } = require('playwright');

let browser = null;

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

function respond(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

async function ensureBrowser() {
  if (!browser) {
    browser = await chromium.launch({ headless: true });
  }
  return browser;
}

async function handleScreenshot(req) {
  const b = await ensureBrowser();
  const context = await b.newContext(
    req.viewport
      ? { viewport: { width: req.viewport.width, height: req.viewport.height } }
      : {}
  );
  const page = await context.newPage();
  try {
    await page.goto(req.url, { waitUntil: 'load', timeout: 30000 });
    const buf = await page.screenshot({ type: 'png', fullPage: false });
    const base64 = buf.toString('base64');
    respond({ id: req.id, ok: true, data: base64 });
  } catch (err) {
    respond({ id: req.id, ok: false, error: err.message });
  } finally {
    await context.close();
  }
}

function extractPreset(preset) {
  switch (preset) {
    case 'links':
      return {
        selector: 'a[href]',
        extract: (el) => ({
          tag: el.tagName.toLowerCase(),
          text: (el.textContent || '').trim().slice(0, 500),
          attributes: { href: el.getAttribute('href') || '' },
        }),
      };
    case 'headings':
      return {
        selector: 'h1,h2,h3,h4,h5,h6',
        extract: (el) => ({
          tag: el.tagName.toLowerCase(),
          text: (el.textContent || '').trim().slice(0, 500),
          attributes: {},
        }),
      };
    case 'tables':
      return {
        selector: 'table',
        extract: (el) => ({
          tag: 'table',
          text: el.innerHTML.slice(0, 2000),
          attributes: {},
        }),
      };
    default:
      return null;
  }
}

async function handleExtract(req) {
  const b = await ensureBrowser();
  const context = await b.newContext();
  const page = await context.newPage();
  try {
    await page.goto(req.url, { waitUntil: 'load', timeout: 30000 });

    let selectorStr = req.selector || null;
    let presetConfig = null;

    if (req.preset) {
      presetConfig = extractPreset(req.preset);
      if (!presetConfig) {
        respond({ id: req.id, ok: false, error: `Unknown preset: ${req.preset}` });
        return;
      }
      selectorStr = presetConfig.selector;
    }

    if (!selectorStr) {
      respond({ id: req.id, ok: false, error: 'No selector or preset provided' });
      return;
    }

    const extractFnBody = presetConfig
      ? presetConfig.extract.toString()
      : `(el) => ({
          tag: el.tagName.toLowerCase(),
          text: (el.textContent || '').trim().slice(0, 500),
          attributes: Object.fromEntries(
            Array.from(el.attributes).map(a => [a.name, a.value])
          ),
        })`;

    const elements = await page.evaluate(
      ({ sel, fnBody }) => {
        const extractFn = eval('(' + fnBody + ')');
        const els = document.querySelectorAll(sel);
        return Array.from(els).slice(0, 200).map(extractFn);
      },
      { sel: selectorStr, fnBody: extractFnBody }
    );

    respond({ id: req.id, ok: true, data: elements });
  } catch (err) {
    respond({ id: req.id, ok: false, error: err.message });
  } finally {
    await context.close();
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

rl.on('line', (line) => {
  handleRequest(line);
});

rl.on('close', async () => {
  if (browser) {
    await browser.close();
  }
  process.exit(0);
});
