// 静态检查：app.js 里有没有「调用了不存在的函数」。
//
// 起因：hlcarry 影子 tab 上线后报 `money is not defined`；而在此之前
// `renderCapitalAlloc is not defined` 已经在 test-account-performance 里挂了。
// 这类错误 node --check 查不出来（语法合法），只有运行时才炸，而运行时往往
// 只在某个 tab 被点开时才炸——正是最不该出事的地方。
//
// 做法：收集全文声明过的顶层名字（function / const / let / var / class），
// 收集所有「裸调用」的标识符（排除 obj.method() 和关键字），差值就是可疑项。
// 允许清单是浏览器/JS 内建，其余一律报出来。
//
// 跑法：node tools/lint-undefined.mjs     有可疑项时退出码 1

import {readFileSync} from 'node:fs';

const path = new URL('../static/app.js', import.meta.url);
const src = readFileSync(path, 'utf8');

// 只去掉注释；**不动字符串**。
// 字符串会被压成一行、行号全错，而且模板字符串 ${...} 里的调用本来就是真调用，
// 剥掉反而漏检。宁可接受少量来自字面量的误报。
const stripped = src
  .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
  .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ');

const declared = new Set();
for (const m of stripped.matchAll(/\bfunction\s+([A-Za-z_$][\w$]*)/g)) declared.add(m[1]);
for (const m of stripped.matchAll(/\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g)) declared.add(m[1]);
for (const m of stripped.matchAll(/\bclass\s+([A-Za-z_$][\w$]*)/g)) declared.add(m[1]);
// 形如 `const {a, b} = ...` 的简单解构
for (const m of stripped.matchAll(/\b(?:const|let|var)\s*\{([^}]*)\}/g)) {
  for (const part of m[1].split(',')) {
    const name = part.split(':').pop().trim().replace(/=.*$/, '').trim();
    if (/^[A-Za-z_$][\w$]*$/.test(name)) declared.add(name);
  }
}

const KEYWORDS = new Set([
  'if', 'for', 'while', 'switch', 'catch', 'function', 'return', 'typeof', 'instanceof',
  'do', 'else', 'try', 'finally', 'throw', 'new', 'delete', 'void', 'in', 'of', 'case',
  'yield', 'await', 'async', 'super', 'this', 'with', 'constructor',
]);

// 浏览器 / JS 内建，允许直接调用
const BUILTINS = new Set([
  'document', 'window', 'console', 'JSON', 'Math', 'Number', 'String', 'Boolean', 'Array',
  'Object', 'Date', 'Promise', 'Symbol', 'BigInt', 'RegExp', 'Error', 'TypeError', 'Map',
  'Set', 'WeakMap', 'WeakSet', 'Proxy', 'Reflect', 'Intl', 'Infinity', 'NaN', 'undefined',
  'parseInt', 'parseFloat', 'isFinite', 'isNaN', 'encodeURIComponent', 'decodeURIComponent',
  'encodeURI', 'decodeURI', 'structuredClone', 'fetch', 'setTimeout', 'setInterval',
  'clearTimeout', 'clearInterval', 'requestAnimationFrame', 'cancelAnimationFrame',
  'alert', 'prompt', 'confirm', 'localStorage', 'sessionStorage', 'navigator', 'location',
  'Image', 'Blob', 'File', 'FileReader', 'URL', 'FormData', 'Headers', 'Request', 'Response',
  'AbortController', 'getComputedStyle', 'matchMedia', 'crypto', 'performance', 'queueMicrotask',
  'btoa', 'atob', 'escape', 'unescape', 'eval', 'Function', 'arguments',
]);

// CSS 字符串里会写成 repeat(...) / minmax(...)，不是 JS 调用。
// 只放确实出现过的；新增误报就再加，别一次性放宽。
const IGNORE = new Set(['repeat', 'minmax']);

const suspicious = new Map();
for (const m of stripped.matchAll(/(^|[^\w$.])([A-Za-z_$][\w$]*)\s*\(/g)) {
  const name = m[2];
  if (KEYWORDS.has(name) || BUILTINS.has(name) || declared.has(name) || IGNORE.has(name)) continue;
  const line = stripped.slice(0, m.index).split('\n').length;
  if (!suspicious.has(name)) suspicious.set(name, []);
  suspicious.get(name).push(line);
}

if (suspicious.size === 0) {
  console.log(`✓ app.js 没有调用未声明的函数（声明 ${declared.size} 个名字）`);
  process.exit(0);
}

console.error(`✗ app.js 里有 ${suspicious.size} 个"调用了不存在的函数"：`);
for (const [name, lines] of [...suspicious].sort((a, b) => a[1][0] - b[1][0])) {
  console.error(`  ${name}()  首次出现在第 ${lines[0]} 行，共 ${lines.length} 处`);
}
process.exit(1);
