#!/usr/bin/env python3
"""Standby image store client: publish linux/amd64 app images to S3-compatible object storage (B2) and fetch them.

One file, copied verbatim into every app repo that publishes a standby image (keep the copies identical; the canonical
copy lives with the standby tooling). Stdlib only: SigV4 is signed here (old curl --aws-sigv4 builds fail on B2).
Used by CI (publish) and by the standby host (fetch). Layout: <prefix>/<name>/<tag>.tar.zst + .sig,
<prefix>/<name>/latest (text: newest tag).
Credentials from the environment, never from argv:
  STANDBY_S3_URL     https://<s3 endpoint>/<bucket>   (region = 2nd label of the host, e.g. s3.us-west-002.x -> us-west-002)
  STANDBY_S3_KEY_ID / STANDBY_S3_KEY
  STANDBY_S3_REGION  optional override
  STANDBY_SIGNING_KEY  (publish) OpenSSH private key; signature = ssh-keygen -Y sign -n $STANDBY_SIG_NAMESPACE
  STANDBY_SIG_NAMESPACE  default ctg-standby (separate from the deploy-drop namespace, so a standby tarball can never
                     be replayed as a deploy drop)
  STANDBY_KEEP       (publish) tags kept per image, default 3
  STANDBY_FORCE=1    (publish) re-upload even when <tag>.tar.zst + .sig already exist (default: skip, e.g. a scheduled
                     rebuild of an unchanged commit)
Usage:
  standby-image.py publish <local image ref> <prefix>/<name> <tag>
        refuses a non-amd64 image; docker save | zstd, sign, upload .sig then .tar.zst, set latest, prune old tags
  standby-image.py exists <prefix>/<name> <tag>   exit 0 when <tag>.tar.zst and its .sig are both stored
  standby-image.py put <local file> <key>        (retries with backoff; the object store drops PUTs in bursts)
  standby-image.py put-text <key> <text>
  standby-image.py get <key> <local file>        (writes <file>.part, renames when complete)
  standby-image.py cat <key>
  standby-image.py ls <prefix>                    (prints: key<TAB>size<TAB>last-modified)
  standby-image.py rm <key>...
  standby-image.py prune <prefix> <keep> [protect-tag...]
        keep the newest <keep> '<tag>.tar.zst' objects (by upload time) under <prefix>, delete older ones and their .sig
"""
import datetime, hashlib, hmac, http.client, os, subprocess, sys, tempfile, time, urllib.parse
import xml.etree.ElementTree as ET

RETRIES = int(os.environ.get('STANDBY_S3_RETRIES', '6'))


def cfg():
    url = os.environ['STANDBY_S3_URL'].rstrip('/')
    u = urllib.parse.urlsplit(url)
    bucket = u.path.strip('/')
    region = os.environ.get('STANDBY_S3_REGION') or u.hostname.split('.')[1]
    return u.scheme, u.hostname, u.port, bucket, region, os.environ['STANDBY_S3_KEY_ID'], os.environ['STANDBY_S3_KEY']


def _sign(key, msg):
    return hmac.new(key, msg.encode(), hashlib.sha256).digest()


def request(method, key='', query=None, body=None, length=None, stream_to=None):
    scheme, host, port, bucket, region, kid, secret = cfg()
    path = '/' + bucket + ('/' + urllib.parse.quote(key, safe='/~') if key else '')
    q = sorted((query or {}).items())
    qs = '&'.join(f"{urllib.parse.quote(k, safe='~')}={urllib.parse.quote(str(v), safe='~')}" for k, v in q)
    now = datetime.datetime.now(datetime.timezone.utc)
    amzdate, day = now.strftime('%Y%m%dT%H%M%SZ'), now.strftime('%Y%m%d')
    payload = 'UNSIGNED-PAYLOAD'
    hdrs = {'host': host + (f':{port}' if port else ''), 'x-amz-content-sha256': payload, 'x-amz-date': amzdate}
    signed = ';'.join(sorted(hdrs))
    canon = '\n'.join([method, path, qs, ''.join(f'{k}:{hdrs[k]}\n' for k in sorted(hdrs)), signed, payload])
    scope = f'{day}/{region}/s3/aws4_request'
    sts = '\n'.join(['AWS4-HMAC-SHA256', amzdate, scope, hashlib.sha256(canon.encode()).hexdigest()])
    k = _sign(_sign(_sign(_sign(('AWS4' + secret).encode(), day), region), 's3'), 'aws4_request')
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    h = dict(hdrs)
    h['Authorization'] = f'AWS4-HMAC-SHA256 Credential={kid}/{scope}, SignedHeaders={signed}, Signature={sig}'
    h['User-Agent'] = 'ctg-standby-s3/1.0'
    if length is not None:
        h['Content-Length'] = str(length)
    conn = (http.client.HTTPSConnection if scheme == 'https' else http.client.HTTPConnection)(host, port, timeout=600)
    conn.request(method, path + ('?' + qs if qs else ''), body=body, headers=h)
    r = conn.getresponse()
    if stream_to is not None and r.status == 200:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            stream_to.write(chunk)
        data = b''
    else:
        data = r.read()
    conn.close()
    return r.status, data


def retry(what, fn):
    err = ''
    for i in range(RETRIES):
        try:
            st, data = fn()
            if st < 300:
                return st, data
            err = f'HTTP {st} {data[:200]!r}'
            if st in (400, 401, 403, 404):
                break
        except (OSError, http.client.HTTPException) as e:
            err = repr(e)
        print(f'standby-s3: {what} attempt {i + 1}/{RETRIES} failed: {err}', file=sys.stderr)
        time.sleep(min(60, 5 * 2 ** i))
    sys.exit(f'standby-s3: {what} failed: {err}')


def put(path, key):
    size = os.path.getsize(path)

    def go():
        with open(path, 'rb') as f:
            return request('PUT', key, body=f, length=size)
    retry(f'put {key}', go)
    print(f'uploaded {key} ({size} bytes)')


def put_text(key, text):
    b = text.encode()
    retry(f'put {key}', lambda: request('PUT', key, body=b, length=len(b)))


def get(key, path):
    def go():
        with open(path + '.part', 'wb') as f:
            st, data = request('GET', key, stream_to=f)
        return st, data
    retry(f'get {key}', go)
    os.replace(path + '.part', path)


def cat(key):
    st, data = retry(f'get {key}', lambda: request('GET', key))
    sys.stdout.write(data.decode())


def ls(prefix):
    out, token = [], None
    ns = '{http://s3.amazonaws.com/doc/2006-03-01/}'
    while True:
        q = {'list-type': '2', 'prefix': prefix}
        if token:
            q['continuation-token'] = token
        st, data = retry(f'list {prefix}', lambda: request('GET', '', query=q))
        root = ET.fromstring(data)
        for c in root.findall(ns + 'Contents'):
            out.append((c.find(ns + 'Key').text, int(c.find(ns + 'Size').text), c.find(ns + 'LastModified').text))
        if root.findtext(ns + 'IsTruncated') == 'true':
            token = root.findtext(ns + 'NextContinuationToken')
        else:
            return out


def rm(keys):
    for k in keys:
        retry(f'delete {k}', lambda: request('DELETE', k))


def prune(prefix, keep, protect):
    objs = ls(prefix)
    tars = sorted([o for o in objs if o[0].endswith('.tar.zst')], key=lambda o: o[2], reverse=True)
    for key, _, _ in tars[keep:]:
        tag = key.rsplit('/', 1)[1][:-len('.tar.zst')]
        if tag in protect:
            continue
        rm([key] + ([key + '.sig'] if any(o[0] == key + '.sig' for o in objs) else []))
        print(f'pruned {key}')


def exists(dest, tag):
    keys = {k for k, _, _ in ls(f'{dest}/{tag}.tar.zst')}
    return f'{dest}/{tag}.tar.zst' in keys and f'{dest}/{tag}.tar.zst.sig' in keys


def publish(ref, dest, tag):
    if os.environ.get('STANDBY_FORCE') != '1' and exists(dest, tag):
        print(f'{dest}/{tag}.tar.zst already stored; nothing to do')
        return
    want = os.environ.get('STANDBY_ARCH', 'amd64')
    arch = subprocess.run(['docker', 'image', 'inspect', ref, '--format', '{{.Architecture}}'], capture_output=True,
                          text=True, check=True).stdout.strip()
    if arch != want:
        sys.exit(f'standby-image: {ref} is {arch}, refusing (standby images are {want})')
    keep = int(os.environ.get('STANDBY_KEEP', '3'))
    ns = os.environ.get('STANDBY_SIG_NAMESPACE', 'ctg-standby')
    d = tempfile.mkdtemp(prefix='standby-image-')
    tar, keyf = os.path.join(d, 'image.tar.zst'), os.path.join(d, 'k')
    try:
        save = subprocess.Popen(['docker', 'save', ref], stdout=subprocess.PIPE)
        with open(tar, 'wb') as f:
            z = subprocess.run(['zstd', '-q', '-T0', '-10', '-c'], stdin=save.stdout, stdout=f)
        save.stdout.close()
        if save.wait() or z.returncode:
            sys.exit('standby-image: docker save | zstd failed')
        fd = os.open(keyf, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.write(fd, (os.environ['STANDBY_SIGNING_KEY'].strip() + '\n').encode()); os.close(fd)
        subprocess.run(['ssh-keygen', '-q', '-Y', 'sign', '-f', keyf, '-n', ns, tar], check=True)
        os.unlink(keyf)
        base = f'{dest}/{tag}.tar.zst'
        put(tar + '.sig', base + '.sig')   # signature first: a reader never sees a tarball without one
        put(tar, base)
        put_text(f'{dest}/latest', tag + '\n')
        prune(dest + '/', keep, {tag})
        print(f'published {ref} -> {base}')
    finally:
        for p in (keyf, tar, tar + '.sig'):
            if os.path.exists(p):
                os.unlink(p)
        os.rmdir(d)


def main(a):
    if not a:
        sys.exit(__doc__)
    c = a[0]
    if c == 'publish':
        publish(a[1], a[2].strip('/'), a[3])
    elif c == 'exists':
        sys.exit(0 if exists(a[1].strip('/'), a[2]) else 1)
    elif c == 'put':
        put(a[1], a[2])
    elif c == 'put-text':
        put_text(a[1], a[2])
    elif c == 'get':
        get(a[1], a[2])
    elif c == 'cat':
        cat(a[1])
    elif c == 'ls':
        for k, s, t in ls(a[1]):
            print(f'{k}\t{s}\t{t}')
    elif c == 'rm':
        rm(a[1:])
    elif c == 'prune':
        prune(a[1], int(a[2]), set(a[3:]))
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    main(sys.argv[1:])
