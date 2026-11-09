import os,sys,json
from pathlib import Path
os.environ["INCLUDE_RECENT"]="1"
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import backtest as b
import numpy as np
import pandas as pd
P=Path(__file__).resolve().parent
out={}
for year in [2023,2024,2025,2026]:
 ix=np.array([i for i in range(33,b.N) if pd.Timestamp(b.ts[i],unit='ms').year==year]);cnt=b.COUNTS[ix-1]
 out[str(year)]={'days':len(ix),'eligible_8plus_days':int((cnt>=8).sum()),'eligible_10plus_days':int((cnt>=10).sum()),'median_universe':float(np.median(cnt)),'first_8plus':next((str(pd.Timestamp(b.ts[i],unit='ms').date()) for i in ix if b.COUNTS[i-1]>=8),None)}
(P/'coverage.json').write_text(json.dumps(out,indent=2));print('COVERAGE',out)
