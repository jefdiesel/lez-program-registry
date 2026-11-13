// RegistryLabel: one line naming a LEZ program, for any Logos UI module.
//
//     RegistryLabel { programId: "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN" }
//
// shows "token v1.2.0 by 7Pq3…kZ9d ✓" (✓ = this machine rebuilt the running code from its
// recorded source), "token v1.2.0 · community" for an entry only others wrote, "· stale" when
// the program now runs other code than registered, "· not deployed" when nothing runs at that
// id yet, and the short id when the program is not
// in the registry. It asks the `program_registry` core module (`program_label`) through the
// Logos bridge, and falls back to the short id when there is no bridge or no module, so it
// never breaks the screen it sits in.
//
// Copy this one file into your module's qml/ folder; it needs only QtQuick. Declare the
// module in your metadata.json: "dependencies": [{"name": "program_registry", "version":
// ">=0.3.0"}]. `info` holds the whole answer (kind, name, version, author, verified, stale,
// upgradeable, …) and `resolved(info)` fires when it arrives. Colours default to neutral
// greys; bind them to your theme (e.g. `textColor: Theme.palette.text`).
// From the LEZ Program Registry, github.com/jefdiesel/lez-program-registry (MIT OR Apache-2.0).

import QtQuick

Item {
    id: label

    property string programId: ""
    property int pixelSize: 14
    property string fontFamily: ""
    property color textColor: "#e8e8e8"
    property color quietColor: "#9a9a9a"
    property color verifiedColor: "#3fb950"
    property color warnColor: "#d29922"
    // How long to wait for the module (ms).
    property int patience: 8000
    // The Logos bridge: Basecamp's `logos` by default. A host may hand in another.
    property var bridge: typeof logos !== "undefined" ? logos : null

    // The module's answer (`program_label`'s value), null until it arrives or if it can't.
    property var info: null
    // "idle", "asking", "ok", "unavailable" (no bridge or module) or "error".
    property string state_: "idle"
    property string error: ""

    signal resolved(var info)

    implicitWidth: line.implicitWidth
    implicitHeight: line.implicitHeight

    function shortId(id) {
        var s = String(id || "")
        return s.length <= 12 ? s : s.slice(0, 4) + "…" + s.slice(-4)
    }

    // Chain text without characters that reorder or hide what follows (bidi overrides and
    // isolates, zero-width, controls). Every Text here is also plain text: no markup.
    function clean(s) {
        return String(s === undefined || s === null ? "" : s).replace(/[\u0000-\u001F\u007F-\u009F\u061C\u200B-\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/g, "")
    }

    function words() {
        var v = info
        if (!v || (v.kind !== "deployer" && v.kind !== "community"))
            return shortId(programId)
        return clean(v.name) + " v" + clean(v.version) + (v.kind === "deployer" ? " by " + clean(v.author_short || shortId(v.author)) : " · community")
    }

    function refresh() {
        info = null
        error = ""
        if (programId === "") {
            state_ = "idle"
            return
        }
        if (!bridge || !bridge.callModuleAsync) {
            state_ = "unavailable"
            return
        }
        state_ = "asking"
        var asked = programId
        bridge.callModuleAsync("program_registry", "program_label", [asked], function (reply) {
            if (asked !== label.programId)
                return
            var r = reply
            for (var depth = 0; depth < 2 && typeof r === "string"; depth++) {
                try { r = JSON.parse(r) } catch (e) { break }
            }
            if (!r || r.success === undefined) {
                label.state_ = "unavailable"
                return
            }
            if (!r.success) {
                label.state_ = "error"
                label.error = String(r.error || "")
                return
            }
            label.info = r.value
            label.state_ = "ok"
            label.resolved(r.value)
        }, patience)
    }

    onProgramIdChanged: refresh()
    onBridgeChanged: refresh()
    Component.onCompleted: refresh()

    Row {
        id: line
        spacing: 0
        Text { textFormat: Text.PlainText;
            text: label.words()
            color: label.info && (label.info.kind === "deployer" || label.info.kind === "community") ? label.textColor : label.quietColor
            font.pixelSize: label.pixelSize
            font.family: label.fontFamily
        }
        Text { textFormat: Text.PlainText;
            visible: !!(label.info && label.info.verified)
            text: " ✓"
            color: label.verifiedColor
            font.pixelSize: label.pixelSize
            font.family: label.fontFamily
            font.bold: true
        }
        Text { textFormat: Text.PlainText;
            visible: !!(label.info && label.info.stale)
            text: " · stale"
            color: label.warnColor
            font.pixelSize: label.pixelSize
            font.family: label.fontFamily
        }
        Text { textFormat: Text.PlainText;
            visible: !!(label.info && label.info.kind !== "unregistered" && label.info.deployed === false)
            text: " \u00b7 not deployed"
            color: label.warnColor
            font.pixelSize: label.pixelSize
            font.family: label.fontFamily
        }
        Text { textFormat: Text.PlainText;
            visible: label.state_ === "asking"
            text: " …"
            color: label.quietColor
            font.pixelSize: label.pixelSize
        }
    }
}
