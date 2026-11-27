/* 把多页官网合并为一个完全自包含的 HTML 文件。
 * 用法：node build-single.mjs  →  dist/index.html
 * 源文件：./index.html 等 5 个页面、./assets/styles.css、./assets/site.js、./assets/*.svg
 */
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = dirname(fileURLToPath(import.meta.url));
const PAGES = [
  { key: 'index', file: 'index.html', title: 'MONO.STT - 本地音视频转录工具' },
  { key: 'pricing', file: 'pricing.html', title: '开源与获取 - MONO.STT' },
  { key: 'changelog', file: 'changelog.html', title: '更新日志 - MONO.STT' },
  { key: 'releases', file: 'releases.html', title: '历史版本 - MONO.STT' },
  { key: 'docs', file: 'docs.html', title: '使用文档 - MONO.STT' }
];
const fileToKey = Object.fromEntries(PAGES.map((p) => [p.file, p.key]));
const DESCRIPTION = 'MONO.STT 是 Windows 与 macOS 上的本地转录工具。导入音频或视频，即可在本机生成带时间戳的逐字稿，支持回听校对、批量导出与飞书云文档。';

const read = (p) => readFile(join(root, p), 'utf8').then((s) => s.replace(/\r\n/g, '\n'));
const uri = (buf) => 'data:image/svg+xml;base64,' + buf.toString('base64');

const css = await read('assets/styles.css');
const faviconURI = uri(await readFile(join(root, 'assets/favicon.svg')));
const waveURI = uri(await readFile(join(root, 'assets/wave-text.svg')));
if (/<\/style/i.test(css)) throw new Error('CSS 中含 </style，需要转义');

/* 站内链接 → hash 路由： #/<page>/<anchor> */
function rewriteLinks(html, currentKey) {
  return html.replace(/href="([^"]*)"/g, (match, url) => {
    if (/^(https?:|mailto:|data:|javascript:)/i.test(url)) return match;
    if (url.startsWith('#')) {
      const anchor = url.slice(1);
      return `href="#/${currentKey}${anchor ? '/' + anchor : ''}"`;
    }
    const m = url.match(/^([A-Za-z0-9_.-]+\.html)(?:#(.+))?$/);
    if (m && fileToKey[m[1]]) {
      return `href="#/${fileToKey[m[1]]}${m[2] ? '/' + m[2] : ''}"`;
    }
    return match;
  });
}

async function pageBody(page) {
  const html = await read(page.file);
  const start = html.indexOf('<body>');
  const end = html.lastIndexOf('</body>');
  let body = html.slice(start + 6, end);
  body = body.replace(/\s*<script src="assets\/site\.js"><\/script>\s*/g, '\n');
  body = body.replace(/src="assets\/wave-text\.svg"/g, `src="${waveURI}"`);
  body = rewriteLinks(body, page.key);
  return body.trim();
}

/* site.js：IIFE → 可重复调用的 initPage()，快捷键只绑定一次 */
let js = await read('assets/site.js');
const before = js;
js = js
  .replace("href: 'index.html#download'", "href: '#/index/download'")
  .replace(/^\(function \(\) \{\n  'use strict';/m, "function initPage() {\n  'use strict';")
  .replace(/\}\)\(\);\s*$/, '}\n')
  .replace(
    "  document.addEventListener('keydown', function (e) {",
    "  if (!window.__monoKeyBound) {\n    window.__monoKeyBound = true;\n    document.addEventListener('keydown', function (e) {"
  )
  .replace(
    "  });\n\n  /* ---------- 开源页：获取方式切换（两种都免费） ---------- */",
    "    });\n  }\n\n  /* ---------- 开源页：获取方式切换（两种都免费） ---------- */"
  );
if (js === before || !js.includes('function initPage()') || js.includes("'index.html#download'")) {
  throw new Error('site.js 改写失败，请检查源文件结构');
}

const titles = JSON.stringify(Object.fromEntries(PAGES.map((p) => [p.key, p.title])));
const router = `
/* ---------- 单文件路由：#/<page>/<anchor> ---------- */
var PAGE_TITLES = ${titles};
var appRoot = document.getElementById('app');
var currentPage = '';

function parseRoute() {
  var raw = location.hash.replace(/^#\\/?/, '');
  var parts = raw.split('/').filter(Boolean).map(function (s) { try { return decodeURIComponent(s); } catch (e) { return s; } });
  var page = Object.prototype.hasOwnProperty.call(PAGE_TITLES, parts[0]) ? parts[0] : 'index';
  return { page: page, anchor: parts[1] || '' };
}

function jump(anchor, behavior) {
  var el = anchor && document.getElementById(anchor);
  if (el) el.scrollIntoView({ behavior: behavior, block: 'start' });
  else window.scrollTo({ top: 0, behavior: behavior });
}

function render() {
  var route = parseRoute();
  var samePage = route.page === currentPage;
  document.title = PAGE_TITLES[route.page];
  if (!samePage) {
    var tpl = document.getElementById('page-' + route.page);
    appRoot.replaceChildren(tpl.content.cloneNode(true));
    currentPage = route.page;
    initPage();
  }
  /* 切换页面立即定位；同页锚点沿用原站的平滑滚动 */
  var behavior = samePage && route.anchor ? 'smooth' : 'instant';
  requestAnimationFrame(function () { jump(route.anchor, behavior); });
}

window.addEventListener('hashchange', render);
/* 重复点击同一 hash 不会触发 hashchange，手动滚动 */
document.addEventListener('click', function (e) {
  var a = e.target && e.target.closest ? e.target.closest('a[href^="#/"]') : null;
  if (!a || a.getAttribute('href') !== location.hash) return;
  e.preventDefault();
  jump(parseRoute().anchor, 'smooth');
});
try { if (!location.hash) history.replaceState(null, '', '#/index'); } catch (err) {}
render();
`;

const templates = [];
for (const page of PAGES) {
  templates.push(`<template id="page-${page.key}">\n${await pageBody(page)}\n</template>`);
}

const out = `<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <link rel="icon" type="image/svg+xml" href="${faviconURI}">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta name="description" content="${DESCRIPTION}">
  <title>${PAGES[0].title}</title>
  <style>
${css}
  </style>
</head>
<body>
<div id="app"></div>
${templates.join('\n')}
<script>
${js}
${router}
</script>
</body>
</html>
`;

await mkdir(join(root, 'dist'), { recursive: true });
const target = join(root, 'dist', 'index.html');
await writeFile(target, out, 'utf8');
console.log('生成：' + target);
console.log('大小：' + Buffer.byteLength(out, 'utf8') + ' bytes');
console.log('页面：' + PAGES.map((p) => p.key).join(', '));
