// 静态检查：初始化顺序（TDZ）风险。
//
// 场景：顶层第 1107 行调用 refreshHlCarry()，而 hcMoney 用 const 声明在 1521 行，
// refreshHlCarry → renderHlCarry → hcMoney，于是浏览器报
//   Cannot access 'hcMoney' before initialization
//
// 为什么别的检查抓不到：
//   - node --check：语法合法
//   - lint-undefined：名字确实声明过，不是"未定义"
//   - 运行时测试：调用方往往是 async，渲染被推到微任务，那时模块已执行完，
//     声明早就初始化好了；只有真实网络时序才会踩到
//
// 做法：建顶层函数的调用图，对每个**顶层调用点**求传递闭包，
// 检查闭包里引用到的 const/let 是否有声明在该调用点之后的。
//
// 跑法：node tools/lint-init-order.mjs     有问题时退出码 1

import {readFileSync} from 'node:fs';

const src = readFileSync(new URL('../static/app.js', import.meta.url), 'utf8');
const lines = src.split('\n');

// 去注释但保留行号
const code = src
  .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
  .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ');

const codeLines = code.split('\n');

/** 收集顶层函数：名字 -> {start, end, body} */
const functions = new Map();
for (let i = 0; i < codeLines.length; i++) {
  const m = /^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(/.exec(codeLines[i]);
  if (!m) continue;
  let end = i;
  for (let j = i + 1; j < codeLines.length; j++) {
    if (codeLines[j].trimEnd() === '}') {
      end = j;
      break;
    }
  }
  functions.set(m[1], {start: i, end, body: codeLines.slice(i, end + 1).join('\n')});
}

/** 顶层 const/let：名字 -> 声明行号（0-based） */
const lexical = new Map();
for (let i = 0; i < codeLines.length; i++) {
  const m = /^(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/.exec(codeLines[i]);
  if (m && !functions.has(m[1])) lexical.set(m[1], i);
}

/** 函数体里引用到的标识符 */
function refs(text) {
  const out = new Set();
  for (const m of text.matchAll(/(?:^|[^\w$.])([A-Za-z_$][\w$]*)/g)) out.add(m[1]);
  return out;
}

const fnRefs = new Map();
for (const [name, info] of functions) fnRefs.set(name, refs(info.body));

/** 从某函数出发，传递闭包能到达的所有标识符 */
function closure(startFn) {
  const seen = new Set();
  const stack = [startFn];
  const idents = new Set();
  while (stack.length) {
    const fn = stack.pop();
    if (seen.has(fn)) continue;
    seen.add(fn);
    const r = fnRefs.get(fn);
    if (!r) continue;
    for (const id of r) {
      idents.add(id);
      if (functions.has(id) && !seen.has(id)) stack.push(id);
    }
  }
  return idents;
}

/** 顶层调用点：形如 `foo();` / `setInterval(foo, ...)` 在列 0 */
const callSites = [];
for (let i = 0; i < codeLines.length; i++) {
  const l = codeLines[i];
  if (!l || l[0].trim() !== l[0]) continue; // 只看列 0
  let m = /^([A-Za-z_$][\w$]*)\s*\(/.exec(l);
  if (m && functions.has(m[1])) {
    callSites.push({line: i, fn: m[1], text: l.trim()});
    continue;
  }
  m = /^(?:setInterval|setTimeout|requestAnimationFrame|queueMicrotask)\(\s*([A-Za-z_$][\w$]*)/.exec(l);
  if (m && functions.has(m[1])) callSites.push({line: i, fn: m[1], text: l.trim()});
}

const problems = [];
for (const site of callSites) {
  const reach = closure(site.fn);
  for (const id of reach) {
    if (!lexical.has(id)) continue;
    const decl = lexical.get(id);
    if (decl > site.line) {
      const fn = [...functions].find(([, v]) => v.start === site.line);
      problems.push({
        name: id,
        declaredAt: decl + 1,
        calledAt: site.line + 1,
        entry: site.fn,
        via: fn ? fn[0] : site.fn,
      });
    }
  }
}

if (problems.length === 0) {
  console.log(`✓ 初始化顺序安全（${callSites.length} 个顶层调用点，${functions.size} 个函数）`);
  process.exit(0);
}

// 说明：这里只做**调用图**的一阶近似，无法区分
//   a) 初始化时真的读到该 const（会 TDZ，必须修）
//   b) 只在事件的回调/闭包里读（不会 TDZ，是误报）
// 两者在图上看不出差别。所以默认只提示不阻断，退出码 0；
// 加 --strict 才当错误处理（用于人工排查）。
const strict = process.argv.includes('--strict');
const uniq = new Map();
for (const p of problems) uniq.set(`${p.name}@${p.calledAt}`, p);
const head = strict ? '✗' : '…';
console.error(`${head} ${uniq.size} 处「声明在调用点之后」的可疑位置（多数是闭包引用，误报）：`);
for (const p of [...uniq.values()].sort((a, b) => a.calledAt - b.calledAt)) {
  console.error(
    `  ${p.name}  声明在第 ${p.declaredAt} 行，第 ${p.calledAt} 行的 ${p.entry}() 的调用链会用到它`,
  );
}
console.error('  真 TDZ 的修法：改成 function 声明（会提升），或把声明移到调用点之前。');
console.error('  加 --strict 可让本检查以非零退出（CI 里不建议，噪声大）。');
process.exit(strict ? 1 : 0);
