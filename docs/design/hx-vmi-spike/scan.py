import sys; sys.path.insert(0,'/work')
from vmi import *
import glob
smap='/work/kallsyms.txt'
S={}
for l in open(smap):
    p=l.split(); S[p[2]]=int(p[0],16)
base=S['_stext']
S={k:v-base+0xffffffff81000000 for k,v in S.items()}  # offset-only profile
t0=time.time()
cr3,idt=regs()
# slide from IDT gate 14 (#PF) handler
g=rd(cr3,idt+14*16,16); lo,sel,ist,typ,mid,hi,_=struct.unpack('<HHBBHII',g)
h=lo|(mid<<16)|(hi<<32)
slide=h-S['asm_exc_page_fault']
assert slide%0x200000==0, hex(slide)
print('kaslr slide',hex(slide))
init=S['init_task']+slide
# discover offsets
blob=rd(cr3,init,0x1800)
comm=blob.find(b'swapper/0\0')
cands=[]
for o in range(0,0x1800,8):
    p=struct.unpack('<Q',blob[o:o+8])[0]
    if p>>47!=0x1ffff: continue
    try:
        n=p;cnt=0
        while cnt<5000:
            n=q(cr3,n); cnt+=1
            if n==init+o: break
        else: continue
        if cnt>3 and cnt<5000: cands.append((o,cnt))
    except Exception: pass
tasks=cands[0][0] if cands else None
print('comm off',hex(comm),'tasks off',hex(tasks),'cands',cands[:3])
# pid offset: systemd pid1
def walk():
    res=[]; n=q(cr3,init+tasks)
    while n!=init+tasks:
        t=n-tasks; c=rd(cr3,t+comm,16).split(b'\0')[0].decode('latin1')
        res.append((t,c)); n=q(cr3,n)
    return res
L=walk()
sysd=[t for t,c in L if c=='systemd'][0]
b=rd(cr3,sysd,0x1800); ib=rd(cr3,init,0x1800)
pid_o=None
for o in range(0x400,0xc00,4):
    pv,tv=struct.unpack('<II',b[o:o+8]); ip,it=struct.unpack('<II',ib[o:o+8])
    if (pv,tv)==(1,1) and (ip,it)==(0,0): pid_o=o;break
print('pid off',hex(pid_o))
def procs():
    return sorted((struct.unpack('<I',rd(cr3,t+pid_o,4))[0],c) for t,c in walk() if struct.unpack('<I',rd(cr3,t+pid_o,4))[0]>0)
P=procs()
open('/work/vmi_procs.txt','w').write('\n'.join(f'{p} {c}' for p,c in P))
# syscall table
sct=S['sys_call_table']+slide; lo_=S['_stext']+slide; hi_=S['_etext']+slide
bad=[];N=460
tbl=rd(cr3,sct,N*8)
for i in range(N):
    v=struct.unpack('<Q',tbl[i*8:i*8+8])[0]
    if not lo_<=v<hi_: bad.append((i,hex(v)))
print('procs',len(P),'syscall entries outside text:',bad,'scan_s',round(time.time()-t0,3))
if len(sys.argv)>1 and sys.argv[1]=='tamper':
    import mmap as M
    pa=xlate(cr3,sct+39*8); w=open(MEM,'r+b'); wm=M.mmap(w.fileno(),0)
    orig=bytes(wm[pa:pa+8]); wm[pa:pa+8]=struct.pack('<Q',0xffffffffc0001000)
    tbl=rd(cr3,sct,N*8); print('after tamper detected:',[(i,hex(struct.unpack('<Q',tbl[i*8:i*8+8])[0])) for i in range(N) if not lo_<=struct.unpack('<Q',tbl[i*8:i*8+8])[0]<hi_])
    wm[pa:pa+8]=orig; print('restored',bytes(wm[pa:pa+8])==orig)
