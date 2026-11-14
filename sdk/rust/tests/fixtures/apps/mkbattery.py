"""Second review's bypass packages (B1 polyglot, B2 edited manifest, archive battery b1-b9),
rebuilt small. Uses the reviewer's tarlib.py. Usage: python3 -I mkbattery.py <outdir>

pub.lgx: a small genuine package (author lns:forumer); evil.lgx: one file changed, hashes
recomputed; poly.lgx: a `manifest.sig` entry whose size field starts with a NUL, so a reader
that stops at the NUL sees pub.lgx and lgx (which skips leading NULs) swallows pub's tar and
reads evil's entries; edited.lgx: pub's files with an edited manifest (same root)."""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tarlib import B, header, entry, end, gz, merkle

out = sys.argv[1]
FILES = [
    ("variants/darwin-arm64/forumer_core_plugin.dylib", b"THE PUBLISHER'S BUILD"),
    ("variants/darwin-arm64/qml/Main.qml", b"Item {}\n"),
]

def manifest(files, **kw):
    m = {"manifestVersion": "0.6.0", "name": "forumer_core", "version": "1.2.0", "author": "lns:forumer",
         "type": "core", "category": "test", "description": "fixture", "icon": "",
         "main": {"darwin-arm64": "forumer_core_plugin.dylib"}, "dependencies": [],
         "hashes": merkle(dict(files))}
    m.update(kw)
    return json.dumps(m, indent=2, sort_keys=True).encode()

def tar(files, m):
    return entry("manifest.json", m) + b"".join(entry(n, d) for n, d in files) + end()

def entries(t):
    at, res = 0, []
    while at < len(t):
        h = t[at:at + B]
        if h == b"\0" * B:
            break
        size = int(h[124:136].split(b"\0")[0].strip() or b"0", 8)
        n = B + (size + B - 1) // B * B
        res.append(t[at:at + n]); at += n
    return res

pub = tar(FILES, manifest(FILES))
evil_files = [(n, d + b"\n// EVIL\n" if n.endswith("Main.qml") else d) for n, d in FILES]
evil = tar(evil_files, manifest(evil_files))
pkgs = {"pub": gz(pub), "evil": gz(evil)}
L = len(pub)
pkgs["poly"] = gz(header("manifest.sig", 0, b"0", size_field=b"\0" + b"%010o" % L + b"\0") + pub + b"".join(entries(evil)) + end())
pkgs["b1-hider-space-nul"] = gz(header("manifest.sig", 0, b"0", size_field=b" \0" + b"%09o" % L + b"\0") + pub + b"".join(entries(evil)) + end())
pkgs["b2-trailing-garbage"] = gz(pub) + b"GARBAGE-AFTER-GZIP"
half = len(pub) // 2 // B * B
pkgs["b3-two-gzip-members"] = gz(pub[:half]) + gz(pub[half:])
pkgs["b4-second-archive"] = gz(pub + b"".join(entries(evil)) + end())
pe = entries(pub)
pkgs["b5-single-zero-block"] = gz(b"".join(pe[:2]) + b"\0" * B + b"".join(pe[2:]) + end())
def reheader(blk, a, b, v):
    h = bytearray(blk[:B]); h[a:b] = v; h[148:156] = b" " * 8; h[148:156] = b"%06o\0 " % sum(h); return bytes(h) + blk[B:]
q = pe[2]
qsize = int(q[124:136].split(b"\0")[0], 8)
pkgs["b6-type7-file"] = gz(b"".join(pe[:2]) + reheader(q, 156, 157, b"7") + end())
pkgs["b7-base256-size"] = gz(b"".join(pe[:2]) + reheader(q, 124, 136, b"\x80" + qsize.to_bytes(11, "big")) + end())
h = bytearray(q[:B]); h[148:156] = b" " * 8; h[148:156] = b"\0%06o\0" % sum(h)
pkgs["b8-checksum-leading-nul"] = gz(b"".join(pe[:2]) + bytes(h) + q[B:] + end())
pkgs["b9-size-junk"] = gz(b"".join(pe[:2]) + reheader(q, 124, 136, b"%011o" % qsize + b"9") + end())
pkgs["edited"] = gz(tar(FILES, manifest(FILES, version="999.0.0", dependencies=[{"name": "evil_core", "version": ">=0.0.1"}], description="Official (verified)")))
# Packages liblgx reads and lezreg must read the same way (the differential test compares them):
# header forms other writers use, all within what lezreg accepts.
def hdr(name, size, kind=b"0", uid=0, mtime=0, uname=b"", gname=b"", dev=b"0000000\0", prefix=b""):
    h = bytearray(B)
    h[0:len(name)] = name
    h[100:108] = b"0000644\0"
    h[108:116] = b"%07o\0" % uid
    h[116:124] = b"%07o\0" % uid
    h[124:136] = b"%011o\0" % size
    h[136:148] = b"%011o\0" % mtime
    h[156:157] = kind
    h[257:265] = b"ustar\x0000"
    h[265:265 + len(uname)] = uname
    h[297:297 + len(gname)] = gname
    h[329:337] = dev
    h[337:345] = dev
    h[345:345 + len(prefix)] = prefix
    h[148:156] = b" " * 8
    h[148:156] = b"%06o\0 " % sum(h)
    return bytes(h)
def ent(path, data, **kw):
    name, prefix = path.encode(), b""
    if len(name) > 100:
        cut = name.rindex(b"/", 0, 156)
        prefix, name = name[:cut], name[cut + 1:]
    return hdr(name, len(data), prefix=prefix, **kw) + data + b"\0" * ((-len(data)) % B)
def variant(files, extra_tail=b"", **kw):
    m = manifest(files)
    return gz(ent("manifest.json", m, **kw) + b"".join(ent(n, d, **kw) for n, d in files) + end() + extra_tail)
LONG = "variants/darwin-arm64/" + "d" * 60 + "/" + "e" * 60 + "/deep.txt"
NAME100 = "variants/darwin-arm64/" + "n" * (100 - len("variants/darwin-arm64/"))
pkgs["ok-typeflag-nul"] = variant(FILES, kind=b"\0")
pkgs["ok-prefix-path"] = variant(FILES + [(LONG, b"deep")])
pkgs["ok-name-100-bytes"] = variant(FILES + [(NAME100, b"exactly 100")])
pkgs["ok-empty-file"] = variant(FILES + [("variants/darwin-arm64/empty", b"")])
pkgs["ok-header-junk"] = variant(FILES, uname=b"someone", gname=b"staff", dev=b"0000017\0")
pkgs["ok-uid-mtime"] = variant(FILES, uid=501, mtime=1700000000)
pkgs["ok-trailing-zero-records"] = variant(FILES, extra_tail=b"\0" * 10240)
for k, v in pkgs.items():
    open(os.path.join(out, k + ".lgx"), "wb").write(v)
print(" ".join(sorted(pkgs)))
