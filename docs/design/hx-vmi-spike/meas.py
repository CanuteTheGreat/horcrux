import sys,subprocess,statistics as st,time,re,os
sys.path.insert(0,'/work')
G=['sshpass','-p','vmitest','ssh','-o','StrictHostKeyChecking=no','-p','2222','root@127.0.0.1']
def g(c,t=300): return subprocess.run(G+[c],capture_output=True,text=True,timeout=t,stdin=subprocess.DEVNULL).stdout
def bench():
    c=g("sysbench cpu --threads=2 --time=10 run | grep 'events per second'")
    m=g("sysbench memory --threads=2 --memory-total-size=20G --memory-block-size=1M run | grep -E 'MiB transferred'")
    return float(c.split()[-1]), float(re.search(r'\(([\d.]+) MiB/sec',m).group(1))
mode=sys.argv[1]; withscan=len(sys.argv)>2
sc=None
if withscan:
    sc=subprocess.Popen(['bash','-c','while true; do python3 /work/scan.py >/dev/null 2>&1; sleep 1; done'])
r=[bench() for _ in range(4)]
if sc: sc.terminate(); subprocess.run(['pkill','-f','while tru[e]'])
print(mode,'scan_every_1s' if withscan else 'no_scan','cpu ev/s',[x[0] for x in r],'mem MiB/s',[x[1] for x in r])
