"""Continue already authorized research after both data stages have finished."""
from pathlib import Path
import subprocess
import sys
import time

root=Path(__file__).resolve().parent
out=root/'options_events'
start=time.monotonic()
while not all((out/n).exists() for n in ['spec_manifest.json','selection_summary.json']):
    if time.monotonic()-start>7200:
        raise TimeoutError('Data stages not complete after two hours; inspect their logs')
    time.sleep(5)
for script in ['options_screen.py','options_audit.py','options_report.py']:
    print('Running',script,flush=True)
    subprocess.run([sys.executable,str(root/script)],check=True)
print('All option screen and audit stages complete.',flush=True)
