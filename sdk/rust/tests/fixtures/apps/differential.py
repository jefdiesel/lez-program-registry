"""Differential test: lezreg's reader against Basecamp's own liblgx over a set of packages.

For every package lezreg reads (lgx::Contents::from_lgx succeeds), liblgx must load it and
extract, for every variant, exactly the files lezreg hashed (the variant's own files plus the
root assets/ merged in), and report a manifest whose every field equals lezreg's. A package
lezreg reads but liblgx would extract differently is a failure: lezreg could call it verified
while Basecamp installs other code.

Usage: python3 -I differential.py <lgx_view binary> <lgxview harness> <workdir> pkg.lgx...
(lgx_view: `cargo build --example lgx_view`; lgxview: a C program over liblgx's C API that
writes lgx_get_manifest_json and lgx_extract's output for every variant, see lgxview.c.)"""
import hashlib, json, os, shutil, subprocess, sys

view, harness, work = sys.argv[1:4]
pkgs = sys.argv[4:]
sha = lambda b: hashlib.sha256(b).hexdigest()
lines = subprocess.run([view] + pkgs, capture_output=True, text=True, check=True).stdout.splitlines()
fails, rows = 0, []
for pkg, line in zip(pkgs, lines):
    lz = json.loads(line)
    out = os.path.join(work, os.path.basename(pkg))
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    r = subprocess.run([harness, pkg, out], capture_output=True, text=True)
    lgx_loaded = "load=ok" in r.stdout
    lgx_valid = "verify=1" in r.stdout
    verdict = "ok"
    if lz["read"]:
        if not lgx_loaded:
            verdict = "lezreg reads it, liblgx doesn't load it (safe: Basecamp won't install it)"
        else:
            files = lz["files"]
            variants = sorted({p.split("/")[1] for p in files if p.startswith("variants/")})
            got_variants = sorted(d for d in os.listdir(out) if os.path.isdir(os.path.join(out, d)))
            problems = []
            if variants != got_variants:
                problems.append(f"variants lezreg {variants} liblgx {got_variants}")
            for v in variants:
                want = {p[len(f"variants/{v}/"):]: h for p, h in files.items() if p.startswith(f"variants/{v}/")}
                want.update({p: h for p, h in files.items() if p.startswith("assets/")})
                got = {}
                base = os.path.join(out, v)
                for dp, _, fs in os.walk(base):
                    for f in fs:
                        full = os.path.join(dp, f)
                        got[os.path.relpath(full, base)] = sha(open(full, "rb").read())
                if want != got:
                    diff = sorted(set(want.items()) ^ set(got.items()))
                    problems.append(f"{v}: extracted files differ: {diff[:4]}")
            raw = open(os.path.join(out, "manifest.json"), "rb").read()
            if lz.get("canonical") == "yes" and sha(raw) != lz["manifest_sha256"]:
                problems.append("lgx-layout package, but liblgx's manifest.json bytes are not the ones lezreg hashed")
            lm = json.loads(raw)
            for k, val in lm.items():
                if lz["manifest"].get(k) != val:
                    problems.append(f"manifest {k}: lezreg {lz['manifest'].get(k)!r} liblgx {val!r}")
            if problems:
                verdict = "DIFFERENT: " + "; ".join(problems)
                fails += 1
    rows.append((os.path.basename(pkg), "read" if lz["read"] else "refused", "loads" if lgx_loaded else "fails", "valid" if lgx_valid else "invalid", lz.get("canonical", "-") == "yes", verdict, "" if lz["read"] else lz["error"][:110]))
for r in rows:
    print(f"{r[0]:<40} lezreg={r[1]:<8} liblgx={r[2]:<6}/{r[3]:<7} lgx-layout={str(r[4]):<5} {r[5]} {r[6]}")
print(f"DIFFERENTIAL: {len(rows)} packages, {fails} where lezreg reads a package liblgx would extract differently")
sys.exit(1 if fails else 0)
