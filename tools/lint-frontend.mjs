#!/usr/bin/env node
// 前端静态检查：抓「调用了但没定义」的函数，以及「引用了但不存在的元素 id」。
// 这类 bug 只在浏览器点开对应页面时才暴露（之前因此白屏过一次）。
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../static/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL(process.argv[2] || "../static/index.html", import.meta.url), "utf8");

// 定义：function / async function / const|let|var 赋值
const defined = new Set();
for (const m of app.matchAll(/(?:^|\n)\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g))
  defined.add(m[1]);
for (const m of app.matchAll(/(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g)) defined.add(m[1]);

const KEYWORDS = new Set(`if else for while do switch case return function async await new delete typeof instanceof in of try catch finally throw
class extends super this null true false undefined void yield static get set import export default break continue const let var`.split(/\s+/));
const GLOBALS = new Set(`Date String Number Math JSON Object Array Set Map WeakMap Promise Error RegExp Symbol Boolean BigInt
parseFloat parseInt isFinite isNaN encodeURIComponent decodeURIComponent structuredClone
setTimeout setInterval clearTimeout clearInterval queueMicrotask fetch alert confirm prompt console
Uint8Array Int8Array Float64Array ArrayBuffer TextEncoder TextDecoder URL URLSearchParams`.split(/\s+/));

// 调用：排除 obj.method( 与关键字
const called = new Set(
  [...app.matchAll(/(?<![.\w$])([A-Za-z_$][\w$]*)\s*\(/g)].map((m) => m[1])
);
// SVG fill="url(#gradient)" is paint syntax, not a JavaScript call.
const CSS_FUNCTIONS = new Set(["url"]);
const missing = [...called]
  .filter((c) => !defined.has(c) && !GLOBALS.has(c) && !KEYWORDS.has(c) && !CSS_FUNCTIONS.has(c))
  .sort();

const ids = new Set([...app.matchAll(/\$\("([\w-]+)"\)/g)].map((m) => m[1]));
const views = new Set([...html.matchAll(/data-view="([a-z]+)"/g)].map((m) => `view-${m[1]}`));
const dynamic = new Set(["mo-ago"]); // 运行时生成
const missingIds = [...ids]
  .filter((i) => !html.includes(`id="${i}"`) && !views.has(i) && !dynamic.has(i))
  .sort();

let bad = false;
if (missing.length) {
  console.error("✗ 调用了未定义的函数:", missing.join(", "));
  bad = true;
}
if (missingIds.length) {
  console.error("✗ 引用了不存在的元素 id:", missingIds.join(", "));
  bad = true;
}
if (bad) process.exit(1);
console.log(`✓ 前端静态检查通过（${defined.size} 个定义 · ${ids.size} 个元素引用）`);
