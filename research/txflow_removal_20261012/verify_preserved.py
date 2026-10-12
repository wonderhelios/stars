"""Compare retained HL code with the removal baseline; no service/network calls."""
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
BASE = '489359e0ba18aa2837acea807fb12c0edce76db3'
def before(path):
    return subprocess.check_output(['git', 'show', f'{BASE}:{path}'], cwd=ROOT, text=True)
def after(path):
    return (ROOT / path).read_text()
def body(source, name):
    match = re.search(r'\bfn ' + name + r'\s*\(', source)
    assert match, name
    start = match.start()
    start = source.index('{', start)
    depth = 0
    for i in range(start, len(source)):
        if source[i] == '{': depth += 1
        elif source[i] == '}': depth -= 1
        if depth == 0: return source[start:i+1]
    raise AssertionError(name)
checks = []
def check(label, condition):
    assert condition, label
    checks.append('PASS: ' + label)
old, new = before('src/live.rs'), after('src/live.rs')
check('merge_reconciliation body is byte-for-byte unchanged', body(old, 'merge_txflow_reconciliation') == body(new, 'merge_reconciliation'))
for name in ['first_receipt_watermark_survives_merge_without_fills', 'txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills']:
    new_name = name.removeprefix('txflow_')
    check(new_name + ' retains all assertions', body(old, name).replace('merge_txflow_reconciliation', 'merge_reconciliation') == body(new, new_name))
check('signal_panel reads its own Store', body(new, 'signal_panel').strip() == '{\n    trader::load_panel(store)\n}')
for path, names in {
    'src/live.rs': ['markets_from_meta','trade_config','can_sign','baseline','reset_capital_for_account_change','load','record_equity_now','refresh_take_profits','reconcile_fills','reconcile_fill_rows','daily_execution'],
    'src/web.rs': ['router','live_config','live_status','live_reset','live_tp','live_capital','live_capital_time','live_records_clear','valid_flow_time'],
    'src/trader.rs': ['build_plan','target_weights','load_panel'],
}.items():
    old, new = before(path), after(path)
    for name in names:
        old_body = body(old, name).replace('merge_txflow_reconciliation', 'merge_reconciliation')
        if name == 'live_config':
            old_body = old_body.replace('if c.txflow { v.floor().clamp(1.0, 20.0) } else { v.clamp(1.0, 20.0) }','v.clamp(1.0, 20.0)')
        check(f'{path}::{name} preserves body (approved branch/name substitutions only)', old_body == body(new, name))
old, new = before('src/trader.rs'), after('src/trader.rs')
check('account_meets_targets only loses venue-specific seed targets', body(old, 'account_meets_targets').replace('plan.txflow_targets.clone()', 'HashMap::new()') == body(new, 'account_meets_targets'))
check('execute calls execute_inner without final account verification', body(new, 'execute').strip() == '{\n    execute_inner(exec, plan, cfg, markets, live).await\n}')
old_inner = body(old, 'execute_inner')
old_inner = re.sub(r'^.*Err\(e\) if exec.is_txflow\(\).*\n', '', old_inner, flags=re.M)
old_inner = re.sub(r'if exec.is_txflow\(\) \{ crate::exchange::describe_txflow\(&st, [^}]+\} else \{ crate::exchange::describe\(&st\) \}', 'crate::exchange::describe(&st)', old_inner)
check('execute_inner matches original HL branches exactly', old_inner == body(new, 'execute_inner'))
routes = lambda s: re.findall(r'^\s*\.route\("/api/live[^\n]+', s, re.M)
check('all 9 HL routes and handlers are identical', routes(before('src/web.rs')) == routes(after('src/web.rs')) and len(routes(after('src/web.rs'))) == 9)
for path in ['src/hl.rs','src/momentum.rs','src/paper.rs']:
    check(path + ' byte-for-byte unchanged', before(path) == after(path))
a = before('src/main.rs'); b = after('src/main.rs')
check('main refresh/research/CLI trade functions unchanged', a[a.index('async fn refresh_loop('):] == b[b.index('async fn refresh_loop('):])
old_pkgs = {(p['name'],p['version'],p.get('source')) for p in tomllib.loads(before('Cargo.lock'))['package']}
new_pkgs = {(p['name'],p['version'],p.get('source')) for p in tomllib.loads(after('Cargo.lock'))['package']}
check('Cargo.lock adds/upgrades no packages', new_pkgs <= old_pkgs)
print('\n'.join(checks))
