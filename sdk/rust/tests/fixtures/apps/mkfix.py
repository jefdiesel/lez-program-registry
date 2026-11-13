# Genuine-root .lgx fixtures for the lezreg app-check regression run (appfix).
import gzip, hashlib, io, json, sys, tarfile
out = sys.argv[1]
H = lambda b: hashlib.sha256(b).hexdigest()
def listing(items):  # {name: hexhash} -> hex or None
    if not items: return None
    return H(b"".join(k.encode() + b"\0" + v.encode() + b"\n" for k, v in sorted(items.items())))
def tree(files):
    hashed = {p: H(d) for p, d in files}
    out = {}; top = {}
    variants = sorted({p.split("/")[1] for p in hashed if p.startswith("variants/") and p.count("/") >= 2})
    vh = {}
    for v in variants:
        h = listing({p[len("variants/" + v) + 1:]: x for p, x in hashed.items() if p.startswith("variants/" + v + "/")})
        if h: vh[v] = h; out["variants/" + v] = h
    if vh: out["variants"] = top["variants"] = listing(vh)
    for t in sorted({p.split("/")[0] for p in hashed} - {"variants"}):
        h = listing({p[len(t) + 1:]: x for p, x in hashed.items() if p.startswith(t + "/")})
        if h: out[t] = top[t] = h
    out["root"] = listing(top)
    return out
FILES = [
    ("variants/darwin-arm64/forumer_ui_plugin.dylib", b"THE PUBLISHER'S BUILD"),
    ("variants/darwin-arm64/qml/Main.qml", b"Item {}"),
    ("variants/linux-amd64/forumer_ui_plugin.so", b"THE PUBLISHER'S LINUX BUILD"),
    ("assets/lidl/forumer.lidl", b"lidl"),
]
def pkg(name, author, edit=lambda m: None, files=FILES):
    m = {"name": "forumer_ui", "version": "1.2.0", "author": author, "type": "ui_qml",
         "hashes": tree(files), "main": {"darwin-arm64": "forumer_ui_plugin.dylib", "linux-amd64": "forumer_ui_plugin.so"}}
    edit(m)
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for p, d in [("manifest.json", json.dumps(m).encode())] + files:
            i = tarfile.TarInfo(p); i.size = len(d); i.mtime = 0; t.addfile(i, io.BytesIO(d))
    data = gzip.compress(raw.getvalue(), mtime=0)
    open(f"{out}/{name}.lgx", "wb").write(data)
    return m["hashes"]["root"], H(data)
info = {}
for name, author in [("genuine", "lns:forumer"), ("genuine-upper", "lns:Forumer"), ("genuine-brand", "lns:brand"),
                     ("genuine-oldfwd", "lns:oldfwd"), ("genuine-dupapp", "lns:dupapp"), ("genuine-twofwd", "lns:twofwd"),
                     ("genuine-nelfwd", "lns:nelfwd"), ("genuine-bomfwd", "lns:bomfwd"), ("genuine-acctfwd", "lns:acctfwd"),
                     ("genuine-sealed", "lns:sealed"), ("genuine-acctname", "lns:11111111111111111111111111111112")]:
    info[name] = pkg(name, author)
def other_main(m): m["main"]["darwin-arm64"] = "qml/Main.qml"
info["edited-sealed"] = pkg("edited-sealed", "lns:sealed", other_main)
# A file directly under variants/: lgx's tree skips it, so it is outside the root.
info["uncovered"] = pkg("uncovered", "lns:forumer", files=FILES + [("variants/evil.dylib", b"NOT COVERED")])
json.dump(info, open(f"{out}/roots.json", "w"), indent=1)
print(json.dumps(info, indent=1))
