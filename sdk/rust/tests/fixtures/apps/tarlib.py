"""Minimal ustar reader/writer with full control of header bytes, plus lgx's Merkle tree."""
import gzip, hashlib, io, json, tarfile

B = 512


def header(name, size, kind=b"0", size_field=None, mode=b"0000644"):
    h = bytearray(B)
    nb = name.encode()
    assert len(nb) <= 100, name
    h[0:len(nb)] = nb
    h[100:100 + len(mode)] = mode
    h[108:115] = b"0000000"
    h[116:123] = b"0000000"
    sf = size_field if size_field is not None else b"%011o\0" % size
    assert len(sf) == 12
    h[124:136] = sf
    h[136:147] = b"00000000000"
    h[156:157] = kind
    h[257:263] = b"ustar\0"
    h[263:265] = b"00"
    h[148:156] = b" " * 8
    s = sum(h)
    h[148:156] = b"%06o\0 " % s
    return bytes(h)


def pad(data):
    return data + b"\0" * ((-len(data)) % B)


def entry(name, data=None, **kw):
    """A file (data bytes) or a directory (data None)."""
    if data is None:
        return header(name, 0, b"5", **kw)
    return header(name, len(data), b"0", **kw) + pad(data)


def end():
    return b"\0" * (2 * B)


def read_lgx(path):
    """[(name, type, bytes)] via Python's tarfile (for genuine inputs only)."""
    t = tarfile.open(path)
    out = []
    for m in t.getmembers():
        out.append((m.name, m.type, t.extractfile(m).read() if m.isfile() else None))
    return out


def sha(b):
    return hashlib.sha256(b).hexdigest()


def listing(items):
    return sha(b"".join(k.encode() + b"\0" + v.encode() + b"\n" for k, v in sorted(items.items())))


def merkle(files):
    """lgx computeMerkleTree over {path: bytes} (files only; manifest.* and top-level files skipped)."""
    res, tops = {}, {}
    hashed = {p: sha(d) for p, d in files.items() if "/" in p and p not in ("manifest.json", "manifest.sig")}
    for top in sorted({p.split("/")[0] for p in hashed}):
        if top == "variants":
            vs = {}
            for v in sorted({p.split("/")[1] for p in hashed if p.startswith("variants/") and p.count("/") >= 2}):
                pre = f"variants/{v}/"
                items = {p[len(pre):]: h for p, h in hashed.items() if p.startswith(pre)}
                if items:
                    vs[v] = res[f"variants/{v}"] = listing(items)
            if vs:
                tops["variants"] = res["variants"] = listing(vs)
        else:
            items = {p[len(top) + 1:]: h for p, h in hashed.items() if p.startswith(top + "/")}
            if items:
                tops[top] = res[top] = listing(items)
    if tops:
        res["root"] = listing(tops)
    return res


def gz(tar):
    bio = io.BytesIO()
    with gzip.GzipFile(fileobj=bio, mode="wb", mtime=0) as g:
        g.write(tar)
    return bio.getvalue()


def build(entries, manifest=None):
    """A plain ustar archive: manifest.json first (if given), then entries [(name, data|None)]."""
    tar = b""
    if manifest is not None:
        tar += entry("manifest.json", json.dumps(manifest, indent=2).encode())
    for name, data in entries:
        tar += entry(name, data)
    return tar + end()
