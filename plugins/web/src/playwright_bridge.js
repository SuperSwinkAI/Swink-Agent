// Playwright bridge script — communicates with Rust via JSON lines on stdin/stdout.
// Usage: node playwright_bridge.js

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
  const context = await b.newContext();
  const page = await context.newPage();
  try {
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
  buildExtractionPlan,
  collectAttributes,
  extractElementData,
};
