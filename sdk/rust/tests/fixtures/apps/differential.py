"""Differential test: lezreg's reader against Basecamp's own liblgx, installing the way lgpm does.

For every package, `lgxinst` (lgxinst.c, linked against the liblgx Basecamp bundles) follows
lgpm's installPlugin: the package_valid gate (lgx_verify_signature), the host variant by
lgx_variant_spellings, lgx_extract into <work>/<variant>/, and manifest.json from
lgx_get_manifest_json. The work directory must be on a case-insensitive volume (APFS's default,
as Basecamp's), so files whose names fold together land on one file as they would in a real
install. `lgx_view` (examples/lgx_view.rs) prints what lezreg read.

Fails if, for a package lezreg reads and liblgx installs, any installed file's BYTES differ from
what lezreg hashed (the variant's files with the root assets merged in), the installed file
count differs, or the installed manifest differs (byte for byte when the package is in lgx's
layout, field by field otherwise).

Covers: what one liblgx build installs from these packages on this host's variant. Doesn't
cover: other variants' install (only the host's is installed), other liblgx versions, the
gzip layer beyond what lezreg refuses, or what Basecamp does after install (loading `main`).

Usage: python3 -I differential.py <lgx_view> <lgxinst> <workdir> pkg.lgx..."""
import hashlib, json, os, shutil, subprocess, sys

view, inst, work = sys.argv[1:4]
pkgs = sys.argv[4:]
sha = lambda b: hashlib.sha256(b).hexdigest()
os.makedirs(work, exist_ok=True)
probe = os.path.join(work, "CaseProbe")
open(probe, "w").close()
assert os.path.exists(os.path.join(work, "caseprobe")), "the work directory must be on a case-insensitive volume"
os.remove(probe)
lines = subprocess.run([view] + pkgs, capture_output=True, text=True, check=True).stdout.splitlines()
fails, rows = 0, []
for i, (pkg, line) in enumerate(zip(pkgs, lines)):
    lz = json.loads(line)
    out = os.path.join(work, f"{i:03d}")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    r = subprocess.run([inst, pkg, out], capture_output=True, text=True).stdout.strip()
    f = dict(kv.split("=", 1) for kv in r.split() if "=" in kv)
    installs = f.get("gate") == "1" and f.get("extract") == "1"
    verdict = "ok"
    if lz["read"] and not installs:
        verdict = "lezreg reads it, lgpm wouldn't install it (safe)"
    elif lz["read"]:
        v = f["variant"]
        base = os.path.join(out, v)
        files = lz["files"]
        prefix = next((f"variants/{w}/" for w in {p.split("/")[1] for p in files if p.startswith("variants/")} if w.lower() == v.lower()), None)
        want = {p[len(prefix):]: h for p, h in files.items() if prefix and p.startswith(prefix)}
        want.update({p: h for p, h in files.items() if p.startswith("assets/")})
        problems = []
        for rel, h in sorted(want.items()):
            path = os.path.join(base, rel)
            if not os.path.isfile(path) or sha(open(path, "rb").read()) != h:
                problems.append(f"{rel}: installed bytes are not the ones lezreg hashed")
        got = [os.path.relpath(os.path.join(dp, x), base) for dp, _, fs in os.walk(base) for x in fs]
        got = [g for g in got if g not in ("manifest.json", "variant")]
        if len(got) != len(want):
            problems.append(f"{len(got)} files installed, lezreg hashed {len(want)}")
        raw = open(os.path.join(base, "manifest.json"), "rb").read()
        if lz.get("canonical") == "yes":
            if sha(raw) != lz["manifest_sha256"]:
                problems.append("lgx-layout package, but the installed manifest.json is not the bytes lezreg hashed")
        else:
            for k, val in json.loads(raw).items():
                if lz["manifest"].get(k) != val:
                    problems.append(f"manifest {k}: lezreg {lz['manifest'].get(k)!r} installed {val!r}")
        if problems:
            verdict = "DIFFERENT: " + "; ".join(problems[:4])
            fails += 1
    rows.append((os.path.basename(pkg), "read" if lz["read"] else "refused", "installs" if installs else "refuses", lz.get("canonical", "-") == "yes", verdict, "" if lz["read"] else lz["error"][:100]))
for r in rows:
    print(f"{r[0]:<40} lezreg={r[1]:<8} lgpm={r[2]:<9} lgx-layout={str(r[3]):<5} {r[4]} {r[5]}")
print(f"DIFFERENTIAL: {len(rows)} packages, {fails} where lezreg reads a package lgpm would install with other bytes")
sys.exit(1 if fails else 0)
