import mmap,struct,socket,json,re,sys,time,subprocess,os
MEM='/work/guestmem'
f=open(MEM,'rb'); mm=mmap.mmap(f.fileno(),0,access=mmap.ACCESS_READ)
def qmp(cmd,**a):
    s=socket.socket(socket.AF_UNIX); s.connect('/work/qmp.sock'); r=s.makefile('rw')
    r.readline(); s.send(b'{"execute":"qmp_capabilities"}\n'); r.readline()
    s.send(json.dumps({"execute":cmd,"arguments":a}).encode()+b'\n')
    while True:
        l=json.loads(r.readline())
        if 'return' in l or 'error' in l: return l
def hmp(c): return qmp('human-monitor-command',**{'command-line':c})['return']
def phys(p,n):
    return mm[p:p+n]
def regs():
    t=hmp('info registers'); 
    d={}
    for k in ('CR3','IDT','LSTAR'):
        pass
    cr3=int(re.search(r'CR3=([0-9a-f]+)',t).group(1),16)
    idt=int(re.search(r'IDT=\s*([0-9a-f]+)',t).group(1),16)
    return cr3,idt
def xlate(cr3,va):
    ent=cr3&0x000ffffffffff000
    for sh in (39,30,21,12):
        i=(va>>sh)&0x1ff
        e=struct.unpack('<Q',phys(ent+i*8,8))[0]
        if not e&1: raise KeyError(hex(va))
        if sh in (30,21) and e&0x80:
            m=(1<<sh)-1; return (e&0x000fffffffe00000 if sh==21 else e&0x000fffffc0000000)|(va&m)
        ent=e&0x000ffffffffff000
    return ent|(va&0xfff)
def rd(cr3,va,n):
    out=b''
    while n>0:
        c=min(n,0x1000-(va&0xfff)); out+=phys(xlate(cr3,va),c); va+=c; n-=c
    return out
def q(cr3,va): return struct.unpack('<Q',rd(cr3,va,8))[0]
