const fs=require('fs'),vm=require('vm');const source=fs.readFileSync('static/app.js','utf8');
const code=source.slice(source.indexOf('function chronRecords'),source.indexOf('function renderMonitor'));
const context={};vm.createContext(context);vm.runInContext(code,context);
const active={coin:'BTC',side:'卖',action:'平仓',size:10,price:110,entry_px:100,result:'成交 2@110',live:true};
const partial=context.parseFill(active,null);if(partial.pnl!==100)throw Error('unexpected');
const pair=context.chronRecords({records:[{...active,size:1,result:'成交 1@110'},{coin:'BTC',pnl:10,tid:99}]});if(pair.realized!==20)throw Error('unexpected');
fs.writeFileSync('research/audit_20261008/ui_evidence.json',JSON.stringify({partial_fill:{planned:10,filled:2,true_pnl:20,displayed_pnl:partial.pnl},duplicate_close:{true_pnl:10,displayed_pnl:pair.realized}},null,2));console.log('Confirmed partial quantity overstatement and duplicated realized PnL');
