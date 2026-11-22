#!/usr/bin/env python3
"""Records the program_registry module's REAL answers to the calls the browser makes, over
logosctl (real Logos Core IPC), into an answers file for tools/render.py.

  record.py <logosctl> <answers.json> [search query]

Run it in a logosctl session (LOGOSCTL_CONFIG_DIR) with program_registry loaded and configured.
Nothing is sent to the chain: prepare_freeze only builds a change; submit is never called.
"""
import json, subprocess, sys

ctl, out = sys.argv[1], sys.argv[2]
search = sys.argv[3] if len(sys.argv) > 3 else ""


def call(method, *args):
    raw = subprocess.run([ctl, "--json", "call", "program_registry", method, *[str(a) for a in args]],
                         capture_output=True, text=True, timeout=120).stdout
    answer = json.loads(raw)
    if "result" not in answer:  # the host gave up (a timeout): recorded as no answer at all
        print(f"  {method}: no answer ({answer.get('code')}: {answer.get('message')})")
        return None
    result = answer["result"]
    return result if isinstance(result, str) else json.dumps(result)


answers = {}


def keep(method, *args):
    reply = call(method, *args)
    if reply is None:
        return None
    answers[method + " " + json.dumps(list(args), separators=(",", ":"))] = reply
    answers.setdefault(method, reply)
    return json.loads(reply)


keep("config_get")
keep("pending_requests")
listed = keep("list_entries", "", 25)
if search:
    keep("search_name", search, 50)
    answers["__search"] = search
for item in listed["value"]["items"][:1]:
    program = (item.get("entry") or {}).get("program") or item["link"]["program"]
    for m in ("resolve_program", "program_label", "third_party_entries", "get_idl"):
        keep(m, program)
    keep("prepare_freeze", json.dumps({"entry": item["address"]}, separators=(",", ":")))
# The installed-copy check (0.1.11): it only draws for an entry with a proven publisher name.
for item in listed["value"]["items"]:
    entry = item.get("entry") or {}
    if entry.get("author_label_proven") is not True:
        continue
    published = keep("publisher_apps", entry["author_label"].strip().removeprefix("lns:"))
    for app in ((published or {}).get("value") or {}).get("apps", []):
        keep("verify_app", app["module"])
    break
json.dump(answers, open(out, "w"), indent=1)
print(f"recorded {len(answers)} answers to {out}")
