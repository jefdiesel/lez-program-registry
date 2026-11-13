"""overlap.lgx: lgx's layout, with a root asset and a variant's own file of the same name and
other bytes (lgx extracts the variant with the root assets merged in, in tar order, so the
variant's copy is what lands). The other files here are the third review's repros
(case-collision.lgx: Plugin.dylib/plugin.dylib; spoof-*.lgx: escape sequences in a module name
and a tar path; record-inject.lgx: a version that breaks out of shell quotes; extra-field.lgx: a
manifest field lgx drops; old/new.lgx: two releases; canon-ok.lgx: a clean package).
usage: python3 -I mkoverlap.py <out.lgx>"""
import gzip, hashlib, json, sys

H = lambda b: hashlib.sha256(b).hexdigest()
listing = lambda items: H(b"".join(k.encode() + b"\0" + v.encode() + b"\n" for k, v in sorted(items.items())))
files = {"assets/icon.png": b"the publisher's icon", "variants/darwin-arm64/assets/icon.png": b"OTHER BYTES", "variants/darwin-arm64/plugin.dylib": b"build"}
leaf = listing({p[len("variants/darwin-arm64/"):]: H(d) for p, d in files.items() if p.startswith("variants/")})
assets = listing({p[len("assets/"):]: H(d) for p, d in files.items() if p.startswith("assets/")})
variants = listing({"darwin-arm64": leaf})
root = listing({"assets": assets, "variants": variants})
m = {"manifestVersion": "0.4.0", "name": "forumer_ui", "version": "1.2.0", "description": "", "author": "lns:forumer", "type": "core",
     "category": "", "icon": "", "dependencies": [], "main": {"darwin-arm64": "plugin.dylib"},
     "hashes": {"root": root, "assets": assets, "variants": variants, "variants/darwin-arm64": leaf}}
mj = json.dumps(m, sort_keys=True, indent=2).encode()

def hdr(path, size, d):
    h = bytearray(512)
    full = path.rstrip("/") + "/" if d else path
    h[0:len(full)] = full.encode()
    h[100:108] = (b"0000755" if d else b"0000644") + b"\0"
    h[108:116] = b"0000000\0"; h[116:124] = b"0000000\0"
    h[124:136] = b"%011o\0" % (0 if d else size); h[136:148] = b"00000000000\0"
    h[148:156] = b"        "; h[156] = ord("5" if d else "0")
    h[257:262] = b"ustar"; h[263:265] = b"00"; h[329:337] = b"0000000\0"; h[337:345] = b"0000000\0"
    h[148:154] = b"%06o" % sum(h); h[154] = 0; h[155] = 32
    return bytes(h)

ents = [("assets/", True, b""), ("assets/icon.png", False, files["assets/icon.png"]), ("manifest.json", False, mj),
        ("variants/", True, b""), ("variants/darwin-arm64/", True, b""), ("variants/darwin-arm64/assets/", True, b""),
        ("variants/darwin-arm64/assets/icon.png", False, files["variants/darwin-arm64/assets/icon.png"]),
        ("variants/darwin-arm64/plugin.dylib", False, files["variants/darwin-arm64/plugin.dylib"])]
t = b"".join(hdr(p, len(d), isd) + d + bytes((-len(d)) % 512) for p, isd, d in ents)
open(sys.argv[1], "wb").write(gzip.compress(t + bytes(1024), mtime=0))
