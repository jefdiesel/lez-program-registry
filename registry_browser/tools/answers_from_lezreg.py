#!/usr/bin/env python3
"""Builds an answers file for tools/render.py from READ-ONLY lezreg output, for when no logosctl
session with program_registry is at hand (tools/record.py is the better source when one is).

  answers_from_lezreg.py <list.json> <network.json> <answers.json> [--show show.json ...] [--idl file] [--search q]

<list.json> is `lezreg --rpc R --registry G list --json` (or tools/live-rc3.json), <network.json>
the module's built-in network profile (module/rust-lib/network-rc3.json). Each --show is
`lezreg ... show <program> --json`. Entries, headers and freshness are the chain's, through the
same SDK serializer the module uses. What is rebuilt here, not read: config_get (from the
network profile), program_label (the module's own rules, without a local verify mark), and
get_idl: "the program has no deployer entry" for a program without one (the module's answer),
and for a deployer entry only from an --idl file whose sha256 matches the entry's.
"""
import argparse, hashlib, json


def ok(v):
    return json.dumps({"success": True, "value": v, "error": None})


def err(e):
    return json.dumps({"success": False, "value": None, "error": e})


def short(a):
    return a[:4] + "…" + a[-4:] if len(a) > 12 else a


def main():
    p = argparse.ArgumentParser()
    p.add_argument("list"); p.add_argument("network"); p.add_argument("out")
    p.add_argument("--show", action="append", default=[]); p.add_argument("--idl", action="append", default=[]); p.add_argument("--search", default="")
    a = p.parse_args()
    listed = json.load(open(a.list))
    net = json.load(open(a.network))
    items = listed["items"]
    answers = {}

    def keep(method, args, reply):
        answers[method + " " + json.dumps(args, separators=(",", ":"))] = reply
        answers.setdefault(method, reply)

    progs = net.get("programs") or {}
    pid = lambda k: (progs.get(k) or {}).get("id")
    keep("config_get", [], ok({"sequencer": net["rpc"], "registry": pid("program_registry") or listed.get("registry"),
                               "storage_api": "http://127.0.0.1:8080", "lns": pid("lns_registry"),
                               "network": {"name": net.get("name"), "chain_id": net.get("chain_id"), "rpc": net.get("rpc"), "source": "built-in", "error": None}}))
    keep("pending_requests", [], ok([]))
    keep("list_entries", ["", 25], ok({"items": items, "next": listed.get("next"), "total": listed.get("total", len(items))}))
    if a.search:
        q = a.search.lower()
        found = [i for i in items if (i["entry"] or i["link"])["name"].lower() == q] + [i for i in items if q in (i["entry"] or i["link"])["name"].lower() and (i["entry"] or i["link"])["name"].lower() != q]
        keep("search_name", [a.search, 50], ok({"items": found, "scanned": len(items), "truncated": False}))
        answers["__search"] = a.search
    idls = {}
    for f in a.idl:
        raw = open(f, "rb").read()
        idls[hashlib.sha256(raw).hexdigest()] = json.loads(raw)
    for f in a.show:
        s = json.load(open(f))
        prog = s["program"]
        third = [t for t in s.get("third_party", []) if isinstance(t.get("kind"), dict) and "third_party" in t["kind"]]
        keep("resolve_program", [prog], ok({k: s[k] for k in ("program", "deployer_entry", "header", "freshness", "third_party_count")}))
        keep("third_party_entries", [prog], ok(third))
        live = (s.get("header") or {}).get("image_id")
        d = s.get("deployer_entry")
        if d:
            label = {"text": f"{d['name']} {d['version']}", "kind": "deployer", "verified": False, "stale": bool(live and live != d["image_id"]),
                     "upgradeable": bool(s.get("header") and not s["header"]["immutable"]), "deployed": bool(live), "name": d["name"], "version": d["version"],
                     "author": d["author"], "author_short": short(d["author"]), "author_label_unproven": d["author_label"]}
            idl = idls.get(d["idl"]["sha256"])
            keep("get_idl", [prog], ok({"status": "ok", "reason": None, "format": "spel", "format_version": idl.get("spec"), "calls": [{"name": i["name"]} for i in idl.get("instructions", [])], "idl": idl}
                                       if idl else {"status": "unavailable", "reason": "no Storage node at 127.0.0.1:8080", "format": None, "format_version": None, "calls": [], "idl": None}))
        elif third:
            c = third[0]["entry"]
            label = {"text": f"{c['name']} {c['version']} (community entry, unproven)", "kind": "community", "verified": False, "stale": bool(live and live != c["image_id"]),
                     "upgradeable": bool(s.get("header") and not s["header"]["immutable"]), "deployed": bool(live), "name": c["name"], "version": c["version"],
                     "author": c["author"], "author_short": short(c["author"]), "author_label_unproven": c["author_label"], "community_count": len(third)}
            keep("get_idl", [prog], err("the program has no deployer entry"))
        else:
            label = {"text": f"{short(prog)} (not in the registry)", "kind": "unregistered", "verified": False, "stale": False, "deployed": bool(live), "community_count": 0}
            keep("get_idl", [prog], err("the program has no deployer entry"))
        keep("program_label", [prog], ok(label))
    json.dump(answers, open(a.out, "w"), indent=1)
    print(f"{len(answers)} answers to {a.out}")


main()
