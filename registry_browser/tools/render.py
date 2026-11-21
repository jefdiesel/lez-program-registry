#!/usr/bin/env python3
"""Renders qml/Main.qml to PNG files without Basecamp, answering its module calls from a file
of REAL program_registry replies (recorded with tools/record.sh over logosctl), the way the
Names app's tools/replay.py does.

  render.py <qml binary> <Logos.Theme import dir> <answers.json> <out dir> [scenario ...]

<Logos.Theme import dir> is the `src/qml` folder of a logos-design-system checkout (the real
tokens). Set QT_QPA_PLATFORM=offscreen to run without a display. A call with no recorded
answer gets no reply, as a module that never answers would: the screen must still draw.

Scenarios: list (the index as it opens), detail (the newest entry's guide page), detail-vest,
detail-family (an older member of a family), detail-registry (a deployer entry), detail-tech
(Technical details open), detail-author (the author's update form, as far down as it goes),
search (a name search), basecamp-list and basecamp-end (1250x700, the test Basecamp's
window; -end scrolled to the bottom), narrow-list, narrow-end and narrow-detail (a window too narrow for both: the guide
page replaces the index), small (a small window with both), busy and still-busy (the list rate-limited: retrying, then given up),
busy and still-busy (the list rate-limited: retrying, then given up), freeze-confirm (the
confirm sheet for Freeze; needs a recorded prepare_freeze), nomodule (no program_registry at all).
Answers can come from tools/record.py (logosctl) or tools/answers_from_lezreg.py (lezreg reads).
"""
import json, os, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))

BRIDGE = r"""
    width: %(width)s; height: %(height)s
    function byName(n) {
        for (var i = 0; i < root.items.length; i++) if ((root.items[i].entry || root.items[i].link).name === n) return root.items[i]
        return root.items[0]
    }
    property var recorded: %(answers)s
    property bool absent: %(absent)s
    property bool fakeBusy: %(busy)s
    property var logos: QtObject {
        function callModuleAsync(module, method, args, callback, timeout) {
            if (root.absent) { Qt.callLater(function() { callback("") }); return }
            // A rate-limited public sequencer, as a program_registry before 0.3.3 reported it.
            if (root.fakeBusy && method === "list_entries") { Qt.callLater(function() { callback(JSON.stringify({ success: false, value: null, error: "cannot reach the sequencer at https://lez.nameswallet.app/: HTTP 429: <!doctype html><!--[if lt IE 7]> <html class=no-js> <![endif]--><head><title>Access denied | lez.nameswallet.app used Cloudflare to restrict access</title></head><body><h1>Error 1015</h1></body></html>" })) }); return }
            var key = method + " " + JSON.stringify(args)
            var reply = root.recorded[key] !== undefined ? root.recorded[key] : root.recorded[method]
            if (reply === undefined) { console.log("unrecorded call: " + key); return }
            Qt.callLater(function() { callback(reply) })
        }
    }
    Timer {
        interval: 1500; running: true
        onTriggered: {
            %(scenario)s
            shot.start()
        }
    }
    Timer {
        id: shot; interval: 1500
        onTriggered: root.grabToImage(function(r) { r.saveToFile("%(out)s"); Qt.quit() })
    }
"""

SCENARIOS = {
    "list": "",
    "detail": "if (root.items.length) root.open(root.items[0])",
    "detail-vest": 'if (root.items.length) root.open(byName("vest"))',
    "detail-family": 'if (root.items.length) root.open(byName("names-evm-bridge-v6"))',
    "detail-registry": 'if (root.items.length) root.open(byName("lez_program_registry"))',
    "detail-tech": 'if (root.items.length) { root.open(byName("vest")); techDetails.open = true }',
    "detail-author": 'if (root.items.length) { root.open(byName("lez_program_registry")); techDetails.open = true; authorTools.open = true }',
    "narrow-list": "",
    "basecamp-list": "",
    "basecamp-detail": 'if (root.items.length) root.open(byName("names-evm-bridge-v7"))',
    "basecamp-end": "list.contentY = list.originY + list.contentHeight - list.height",
    "narrow-end": "list.contentY = list.originY + list.contentHeight - list.height",
    "narrow-detail": 'if (root.items.length) root.open(byName("vest"))',
    "small": 'if (root.items.length) root.open(byName("names-evm-bridge-v7"))',
    "search": 'root.query = "%(search)s"; root.loadList(false)',
    "freeze-confirm": 'if (root.items.length) { root.open(root.items[0]); root.prepare("prepare_freeze", {}) }',
    "nomodule": "",
    "busy": "",
    "still-busy": "root.busyTries = 4; root.busyUntil = 0; root.loadList(false)",
}


SIZES = {"basecamp-list": (1250, 700), "basecamp-detail": (1250, 700), "basecamp-end": (1250, 700), "narrow-end": (720, 900), "narrow-list": (720, 900), "narrow-detail": (720, 900), "small": (1000, 700), "detail-tech": (1280, 1500), "detail-author": (1280, 2300)}


def main():
    qml, theme_dir, answers_path, out_dir = sys.argv[1:5]
    wanted = sys.argv[5:] or list(SCENARIOS)
    answers = json.load(open(answers_path))
    search = answers.pop("__search", "")
    source = open(os.path.join(HERE, "..", "qml", "Main.qml")).read()
    marker = "Item {\n    id: root\n"
    assert marker in source, "Main.qml no longer starts the way the harness expects"
    os.makedirs(out_dir, exist_ok=True)
    # The theme first, then whatever the Qt install needs (a bare `qml` from nixpkgs needs its
    # own lib/qt-6/qml listed). Logos.Theme keeps a settings file under HOME: point HOME at a
    # scratch folder when rendering.
    imports = os.pathsep.join(p for p in [theme_dir, os.environ.get("QML_IMPORT_PATH", "")] if p)
    env = dict(os.environ, QML_IMPORT_PATH=imports, QT_QPA_PLATFORM=os.environ.get("QT_QPA_PLATFORM", "offscreen"))
    failed = 0
    for scenario in wanted:
        out = os.path.abspath(os.path.join(out_dir, scenario + ".png"))
        if os.path.exists(out):
            os.unlink(out)
        bridge = BRIDGE % {"answers": json.dumps(answers), "absent": "true" if scenario == "nomodule" else "false", "busy": "true" if scenario in ("busy", "still-busy") else "false",
                           "scenario": SCENARIOS[scenario] % {"search": search}, "out": out,
                           "width": SIZES.get(scenario, (1280, 900))[0], "height": SIZES.get(scenario, (1280, 900))[1]}
        # Beside Main.qml, so its relative imports (logic.mjs, RegistryLabel.qml) resolve.
        with tempfile.NamedTemporaryFile("w", suffix=".qml", delete=False, dir=os.path.join(HERE, "..", "qml")) as handle:
            handle.write(source.replace(marker, marker + bridge, 1))
        try:
            result = subprocess.run([qml, handle.name], capture_output=True, text=True, timeout=60, env=env)
            problems = [l for l in result.stderr.splitlines() if l.strip() and "qt.qpa.fonts" not in l and "propagateSizeHints" not in l]
        except subprocess.TimeoutExpired:
            problems = ["did not quit within 60 s"]
        finally:
            os.unlink(handle.name)  # never leave it beside Main.qml: the package build copies qml/ whole
        ok = os.path.exists(out)
        failed += not ok
        print(scenario, "ok" if ok else "FAILED", *problems[:8], sep="\n  " if problems else " ")
    sys.exit(1 if failed else 0)


main()
