#!/usr/bin/env python3
"""Tiny QMP client: qmp.py '{"execute":"query-status"}' ..."""
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1] if sys.argv[1].endswith('.sock') else '/w/qmp.sock')
f = s.makefile('rw')
def rd():
    while True:
        m = json.loads(f.readline())
        if 'event' in m: continue
        return m
json.loads(f.readline())
f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); rd()
for c in sys.argv[1:]:
    if c.endswith('.sock'): continue
    f.write(c + '\n'); f.flush(); print(json.dumps(rd()))
