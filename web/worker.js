// Cloudflare Worker for the corvid short-URL installer.
//
// Routes one origin (e.g. corvid.sh) three ways based on User-Agent:
//   - curl/wget/fetch      -> serves install/install.sh from GitHub raw
//   - PowerShell           -> serves install/install.ps1 from GitHub raw
//   - browser              -> serves the inlined landing page (HTML below)
//
// Explicit paths still work for testing or override:
//   /install.sh, /install.ps1   -> always serve that script
//   /raw/<branch>/<path>        -> proxy any file from the repo (for debugging)
//
// Deploy: `wrangler deploy` from this directory after running `wrangler login`.
// Bind a custom domain in the Cloudflare dashboard (Workers & Pages -> your
// worker -> Triggers -> Custom Domains) so the apex (e.g. corvid.sh) routes here.

const REPO   = 'Micrurus-Ai/Corvid-lang';
const BRANCH = 'main';

const SH_URL = `https://raw.githubusercontent.com/${REPO}/${BRANCH}/install/install.sh`;
const PS_URL = `https://raw.githubusercontent.com/${REPO}/${BRANCH}/install/install.ps1`;

const SHELL_AGENTS = /\b(curl|wget|httpie|fetch|libfetch|aria2|powershell\/[0-9]|wininet)\b/i;
const PS_AGENTS    = /\b(powershell|pwsh|windowspowershell|mozilla\/[0-9].* msie .* trident)\b/i;
// PowerShell's Invoke-WebRequest UA looks like
//   "Mozilla/5.0 (Windows NT 10.0; Microsoft Windows 10.0.19045; en-US) PowerShell/7.4.0"
// so we test for "powershell" specifically before falling through to shell.

export default {
    async fetch(request) {
        const url = new URL(request.url);
        const ua  = request.headers.get('user-agent') || '';
        const path = url.pathname.replace(/\/+$/, '') || '/';

        // Explicit script paths
        if (path === '/install.sh')  return proxyScript(SH_URL, 'text/x-shellscript');
        if (path === '/install.ps1') return proxyScript(PS_URL, 'text/plain');

        // Debug passthrough: /raw/<branch>/<file...>
        if (path.startsWith('/raw/')) {
            const rest = path.slice(5);
            const target = `https://raw.githubusercontent.com/${REPO}/${rest}`;
            return proxyScript(target, 'text/plain');
        }

        // Root and /install: dispatch by User-Agent
        if (path === '/' || path === '/install') {
            if (PS_AGENTS.test(ua) && /powershell|pwsh/i.test(ua)) {
                return proxyScript(PS_URL, 'text/plain');
            }
            if (SHELL_AGENTS.test(ua)) {
                return proxyScript(SH_URL, 'text/x-shellscript');
            }
            return new Response(LANDING_HTML, {
                headers: {
                    'content-type': 'text/html; charset=utf-8',
                    'cache-control': 'public, max-age=300',
                },
            });
        }

        return new Response('Not found.\n\nTry:\n  curl -fsSL https://corvid.sh | sh\n  irm https://corvid.sh | iex\n', { status: 404 });
    },
};

async function proxyScript(srcUrl, contentType) {
    const upstream = await fetch(srcUrl, {
        cf: { cacheTtl: 300, cacheEverything: true },
    });
    if (!upstream.ok) {
        return new Response(`upstream ${upstream.status}: ${srcUrl}\n`, { status: 502 });
    }
    const body = await upstream.text();
    return new Response(body, {
        headers: {
            'content-type': `${contentType}; charset=utf-8`,
            'cache-control': 'public, max-age=300',
            'x-corvid-source': srcUrl,
        },
    });
}

const LANDING_HTML = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Install Corvid</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="description" content="One-line installer for the Corvid programming language.">
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; padding: 0;
    font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    background: #0b0d10; color: #e6e7eb;
    min-height: 100vh; display: flex; flex-direction: column;
  }
  main {
    max-width: 720px; margin: 0 auto; padding: 64px 24px 96px; flex: 1;
  }
  h1 {
    font-size: 56px; font-weight: 800; letter-spacing: -0.03em;
    margin: 0 0 8px;
    background: linear-gradient(135deg, #ffffff 0%, #8aa0ff 100%);
    -webkit-background-clip: text; background-clip: text; color: transparent;
  }
  .tag { color: #9aa0a6; font-size: 18px; margin: 0 0 40px; }
  .tabs { display: flex; gap: 4px; margin-bottom: 12px; }
  .tab {
    background: #1a1d22; color: #9aa0a6; border: 1px solid #2a2e35;
    padding: 8px 16px; border-radius: 8px 8px 0 0; cursor: pointer;
    font: inherit; font-size: 14px;
  }
  .tab[aria-selected="true"] { background: #11141a; color: #e6e7eb; border-bottom-color: #11141a; }
  .panel {
    background: #11141a; border: 1px solid #2a2e35; border-radius: 0 8px 8px 8px;
    padding: 16px; position: relative;
  }
  pre {
    margin: 0; font-family: ui-monospace, "JetBrains Mono", Consolas, monospace;
    font-size: 14px; line-height: 1.6; color: #e6e7eb;
    overflow-x: auto;
  }
  .copy {
    position: absolute; top: 12px; right: 12px;
    background: #2a2e35; color: #c9cdd4; border: 0; border-radius: 6px;
    padding: 6px 10px; font: inherit; font-size: 12px; cursor: pointer;
  }
  .copy:hover { background: #3a3f48; }
  .copy.ok { background: #1f6f43; color: #fff; }
  h2 {
    font-size: 14px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em;
    color: #9aa0a6; margin: 48px 0 12px;
  }
  ul { padding-left: 20px; line-height: 1.7; color: #c9cdd4; }
  code {
    font-family: ui-monospace, "JetBrains Mono", Consolas, monospace;
    font-size: 13px; background: #1a1d22; padding: 2px 6px; border-radius: 4px;
    color: #e6e7eb;
  }
  a { color: #8aa0ff; text-decoration: none; }
  a:hover { text-decoration: underline; }
  footer {
    border-top: 1px solid #2a2e35; padding: 24px;
    color: #6b7280; font-size: 13px; text-align: center;
  }
</style>
</head>
<body>
<main>
  <h1>Corvid</h1>
  <p class="tag">A general-purpose language for AI-native software. Install with one line.</p>

  <div class="tabs" role="tablist">
    <button class="tab" role="tab" id="tab-unix" aria-selected="true" aria-controls="panel-unix">macOS / Linux</button>
    <button class="tab" role="tab" id="tab-win"  aria-selected="false" aria-controls="panel-win">Windows</button>
  </div>

  <div class="panel" id="panel-unix" role="tabpanel" aria-labelledby="tab-unix">
    <button class="copy" data-target="cmd-unix">Copy</button>
    <pre id="cmd-unix">curl -fsSL https://corvid.sh | sh</pre>
  </div>
  <div class="panel" id="panel-win" role="tabpanel" aria-labelledby="tab-win" hidden>
    <button class="copy" data-target="cmd-win">Copy</button>
    <pre id="cmd-win">irm https://corvid.sh | iex</pre>
  </div>

  <h2>What this does</h2>
  <ul>
    <li>Detects your OS and CPU architecture.</li>
    <li>Downloads the matching prebuilt <code>corvid</code> binary from the latest GitHub Release.</li>
    <li>Installs to <code>~/.corvid</code> (or <code>%USERPROFILE%\\.corvid</code> on Windows).</li>
    <li>Adds <code>~/.corvid/bin</code> to your <code>PATH</code> and runs <code>corvid doctor</code>.</li>
    <li>If no prebuilt is available for your platform, falls back to a <code>cargo install</code> from source.</li>
  </ul>

  <h2>Pin a specific version</h2>
  <pre style="background:#11141a;border:1px solid #2a2e35;border-radius:8px;padding:16px;">CORVID_VERSION=v0.1.0 curl -fsSL https://corvid.sh | sh</pre>

  <h2>Source</h2>
  <ul>
    <li><a href="https://github.com/${REPO}">github.com/${REPO}</a> &mdash; the language and compiler</li>
    <li><a href="https://github.com/${REPO}/tree/${BRANCH}/install">/install</a> &mdash; the installer scripts</li>
    <li><a href="/install.sh">corvid.sh/install.sh</a> &middot; <a href="/install.ps1">corvid.sh/install.ps1</a> &mdash; raw scripts</li>
  </ul>
</main>

<footer>
  Dual-licensed MIT or Apache-2.0.
</footer>

<script>
  // Auto-select tab based on platform
  const isWin = /Win/i.test(navigator.platform || navigator.userAgent || '');
  if (isWin) {
    document.getElementById('tab-unix').setAttribute('aria-selected','false');
    document.getElementById('tab-win').setAttribute('aria-selected','true');
    document.getElementById('panel-unix').hidden = true;
    document.getElementById('panel-win').hidden = false;
  }

  // Tab switching
  for (const tab of document.querySelectorAll('.tab')) {
    tab.addEventListener('click', () => {
      for (const t of document.querySelectorAll('.tab')) t.setAttribute('aria-selected', String(t === tab));
      for (const p of document.querySelectorAll('[role=tabpanel]')) p.hidden = (p.getAttribute('aria-labelledby') !== tab.id);
    });
  }

  // Copy buttons
  for (const btn of document.querySelectorAll('.copy')) {
    btn.addEventListener('click', async () => {
      const text = document.getElementById(btn.dataset.target).textContent.trim();
      try {
        await navigator.clipboard.writeText(text);
        const original = btn.textContent;
        btn.textContent = 'Copied'; btn.classList.add('ok');
        setTimeout(() => { btn.textContent = original; btn.classList.remove('ok'); }, 1200);
      } catch (e) {}
    });
  }
</script>
</body>
</html>`;
