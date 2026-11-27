import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const configPath = new URL('./brand.config.json', import.meta.url);
const files = [
  'index.html', 'pricing.html', 'changelog.html', 'releases.html', 'docs.html',
  'assets/site.js'
];

const cfg = JSON.parse(await readFile(configPath, 'utf8'));
let total = 0;

for (const pair of cfg.pairs) {
  if (!pair.to || pair.from === pair.to) continue;
  if (!pair.from) continue;
  let count = 0;
  for (const name of files) {
    const url = new URL(name, import.meta.url);
    const text = await readFile(url, 'utf8');
    if (!text.includes(pair.from)) continue;
    const hits = text.split(pair.from).length - 1;
    await writeFile(url, text.split(pair.from).join(pair.to), 'utf8');
    count += hits;
  }
  console.log(`${pair.label}: ${pair.from} → ${pair.to}（${count} 处）`);
  pair.from = pair.to;
  total += count;
}

await writeFile(configPath, JSON.stringify(cfg, null, 2) + '\n', 'utf8');
console.log(total ? `完成，共替换 ${total} 处。` : '没有需要替换的内容：请先把 brand.config.json 里的 to 改成你的项目信息。');
console.log('项目目录：' + root);
