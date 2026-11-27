import sys,time,os,statistics as st; sys.path.insert(0,'/work')
from vmi import *
ts=[]
for _ in range(20):
    t=time.time(); qmp('stop'); t1=time.time(); qmp('cont'); ts.append((time.time()-t)*1000)
print('QMP stop+cont round trip ms: mean %.1f max %.1f'%(st.mean(ts),max(ts)))
# paused snapshot: dump-guest-memory
t=time.time(); qmp('stop'); r=qmp('dump-guest-memory',paging=False,protocol='file:/work/dump.elf'); 
while True:
    s=qmp('query-dump')['return']
    if s['status']!='active': break
dt=time.time()-t; qmp('cont'); print('paused full dump 2G: %.2fs status=%s size=%dMB'%(dt,s['status'],os.path.getsize('/work/dump.elf')>>20)); os.remove('/work/dump.elf')
# live scan: repeated scans while guest runs, time each
import subprocess
ts=[]
for _ in range(15):
    t=time.time(); subprocess.run(['python3','/work/scan.py'],capture_output=True); ts.append(time.time()-t)
print('live scan wall s (python prototype, no pause): mean %.2f max %.2f'%(st.mean(ts),max(ts)))
