// Program Registry for Basecamp: browse and search the LEZ Program Registry, read an entry
// (deployer or community, fresh or stale, its IDL, this machine's verify mark), and, for an
// entry your lezreg wallet authors, update or freeze it.
//
// A front end on the `program_registry` core module, which reads the sequencer itself; nothing
// here talks to the network. Changes are prepared by the module (`prepare_*`), shown on a
// confirm sheet, and signed by your own `lezreg` CLI wallet (`submit`).
//
// Controls are drawn here, as the Names app does, so it looks the same whatever style the host
// applies; colours come from the shared Logos theme.

import QtQuick
import QtQuick.Layouts
import Logos.Theme
import "logic.mjs" as Logic

Item {
    id: root

    // ── State ───────────────────────────────────────────────────────────
    property var config: null          // config_get: {sequencer, registry, storage_api, …}
    property var items: []             // listed entries: {address, seq, kind, link, entry}
    property string next: ""           // list_entries' cursor for the next page
    property int total: 0
    property string query: ""          // the search shown, "" for the full list
    property bool loading: false
    property string problem: ""        // why the list could not be read
    property var selected: null        // the listed item open on the right
    property var resolved: null        // resolve_program for its program
    property var idl: null             // get_idl
    property var label: null           // program_label (the local verify mark)
    property var community: []         // third_party_entries
    property bool settingsOpen: false
    property string editVersion: ""
    property string editDescription: ""
    property var change: null          // a prepared change on the confirm sheet
    property var job: null             // the last submitted change
    property var requests: []          // changes other apps submitted, waiting for approval
    property string notice: ""

    readonly property bool bridgeReady: typeof logos !== "undefined" && !!logos && !!logos.callModuleAsync

    // ── Look (the Names app's tokens) ───────────────────────────────────
    readonly property color slate: Theme.palette.background
    readonly property color card: Theme.isDark ? Theme.palette.backgroundSecondary : Theme.palette.surfaceRaised
    readonly property color raised: Theme.isDark ? Theme.palette.surface : Theme.palette.backgroundButton
    readonly property color line: Theme.palette.border
    readonly property color ink: Theme.palette.text
    readonly property color dim: Theme.palette.textSecondary
    readonly property color faint: Theme.palette.textPlaceholder
    readonly property color brass: Theme.isDark ? Theme.palette.warning : Theme.palette.primary
    readonly property color alarm: Theme.palette.error
    readonly property color good: Theme.palette.success
    function px(size) { return Math.round(size * (Theme.typography.scale || 1)) }
    readonly property string sans: "Avenir Next"
    readonly property string mono: "Menlo"

    // ── The module ──────────────────────────────────────────────────────
    /// Asks program_registry; exactly one of done(value) and failed(message, code) runs, later.
    function ask(method, args, done, failed, patience) {
        if (!bridgeReady) {
            Qt.callLater(function() { failed("This screen needs to run inside Basecamp.", "no_bridge") })
            return
        }
        logos.callModuleAsync("program_registry", method, args, function(reply) {
            var a = Logic.answer(reply)
            if (a.ok) done(a.value)
            else failed(a.error, a.code)
        }, patience || 20000)
    }

    function loadConfig() {
        ask("config_get", [], function(v) {
            config = v
            sequencerField.text = v.sequencer || ""
            registryField.text = v.registry || ""
            if (v.registry) loadList(false)
            else { settingsOpen = true; problem = "Set the sequencer and the registry's program id to start." }
        }, function(e) { problem = e })
    }

    function saveConfig() {
        ask("config_set", [JSON.stringify({ sequencer: sequencerField.text.trim(), registry: registryField.text.trim() })], function(v) {
            config = v
            settingsOpen = false
            loadList(false)
        }, function(e) { notice = e })
    }

    function loadList(more) {
        loading = true
        problem = ""
        if (query !== "") {
            ask("search_name", [query, 50], function(v) {
                items = v.items; next = ""; total = v.items.length; loading = false
                if (v.truncated) notice = "The search looked at the newest " + v.scanned + " entries."
            }, function(e) { problem = e; loading = false })
            return
        }
        ask("list_entries", [more ? next : "", 25], function(v) {
            items = more ? items.concat(v.items) : v.items
            next = v.next || ""
            total = v.total
            loading = false
        }, function(e) { problem = e; loading = false })
    }

    function open(item) {
        selected = item
        resolved = null; idl = null; label = null; community = []
        change = null
        var program = item.entry ? item.entry.program : item.link.program
        editVersion = item.entry ? item.entry.version : ""
        editDescription = item.entry ? item.entry.description : ""
        ask("resolve_program", [program], function(v) { resolved = v }, function(e) { notice = e })
        ask("program_label", [program], function(v) { label = v }, function(e) {})
        ask("third_party_entries", [program], function(v) { community = v }, function(e) {})
        ask("get_idl", [program], function(v) { idl = v }, function(e) { idl = { status: "unavailable: " + e, idl: null } }, 30000)
    }

    function prepare(method, fields) {
        notice = ""
        var input = Object.assign({ entry: selected.address }, fields)
        ask(method, [JSON.stringify(input)], function(v) { change = v }, function(e, code) { notice = e + (code ? " (" + code + ")" : "") })
    }

    function submitChange() {
        var c = change
        change = null
        ask("submit", [JSON.stringify(c)], function(v) { job = v; pollJob() }, function(e, code) { notice = e + (code ? " (" + code + ")" : "") })
    }

    function pollJob() {
        if (!job || job.state !== "running") return
        ask("submit_status", [job.job], function(v) {
            job = v
            if (v.state === "running") jobTimer.start()
            else if (v.state === "applied") open(selected)
        }, function(e) { notice = e })
    }

    Timer { id: jobTimer; interval: 2000; onTriggered: root.pollJob() }

    // Other apps can't sign through the registry: their changes wait here for the person.
    function loadRequests() {
        ask("pending_requests", [], function(v) { requests = v }, function(e) {})
    }
    function answerRequest(id, approve) {
        ask(approve ? "approve_request" : "deny_request", [id], function(v) {
            if (approve) { job = v; pollJob() }
            loadRequests()
        }, function(e, code) { notice = e + (code ? " (" + code + ")" : ""); loadRequests() })
    }
    Timer { interval: 5000; running: root.bridgeReady; repeat: true; triggeredOnStart: true; onTriggered: root.loadRequests() }

    Component.onCompleted: loadConfig()

    // ── Controls ────────────────────────────────────────────────────────
    component Action: Rectangle {
        id: action
        property string label: ""
        property bool strong: false
        property bool grave: false
        property bool enabledNow: true
        readonly property color fill: grave ? root.alarm : root.brass
        signal pressed()
        implicitWidth: actionText.implicitWidth + 24
        implicitHeight: 34
        radius: 6
        color: !enabledNow ? "transparent" : strong ? (area.containsMouse ? Qt.lighter(fill, 1.08) : fill) : (area.containsMouse ? root.raised : "transparent")
        border.width: strong && enabledNow ? 0 : 1
        border.color: grave ? root.alarm : root.line
        opacity: enabledNow ? 1 : 0.55
        Text {
            id: actionText
            anchors.centerIn: parent
            text: action.label
            color: action.strong && action.enabledNow ? root.slate : action.grave ? root.alarm : root.ink
            font.family: root.sans
            font.pixelSize: root.px(15)
            font.weight: Font.DemiBold
        }
        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            enabled: action.enabledNow
            cursorShape: Qt.PointingHandCursor
            onClicked: action.pressed()
        }
    }

    component Field: Rectangle {
        id: field
        property alias text: input.text
        property string placeholder: ""
        property bool monospace: false
        signal accepted()
        implicitHeight: 34
        radius: 6
        color: root.slate
        border.width: 1
        border.color: input.activeFocus ? root.brass : root.line
        TextInput {
            id: input
            anchors.fill: parent
            anchors.leftMargin: 10
            anchors.rightMargin: 10
            verticalAlignment: TextInput.AlignVCenter
            color: root.ink
            clip: true
            font.family: field.monospace ? root.mono : root.sans
            font.pixelSize: root.px(14)
            selectByMouse: true
            onAccepted: field.accepted()
        }
        Text {
            anchors.fill: input
            anchors.leftMargin: 10
            verticalAlignment: Text.AlignVCenter
            visible: input.text === "" && !input.activeFocus
            text: field.placeholder
            color: root.faint
            font.family: root.sans
            font.pixelSize: root.px(14)
        }
    }

    /// Deployer (proven) or Community (unproven).
    component KindBadge: Rectangle {
        property string kind: "deployer"
        readonly property var badge: Logic.kindBadge(kind)
        implicitWidth: badgeText.implicitWidth + 14
        implicitHeight: 22
        radius: 11
        color: badge.proven ? Qt.rgba(root.good.r, root.good.g, root.good.b, 0.16) : Qt.rgba(root.brass.r, root.brass.g, root.brass.b, 0.16)
        Text {
            id: badgeText
            anchors.centerIn: parent
            text: parent.badge.text
            color: parent.badge.proven ? root.good : root.brass
            font.family: root.sans
            font.pixelSize: root.px(12)
            font.weight: Font.DemiBold
        }
    }

    component Row2: RowLayout {
        property string key: ""
        property string value: ""
        property bool monospace: false
        property color tone: root.ink
        Layout.fillWidth: true
        spacing: 12
        Text { text: parent.key; color: root.dim; font.family: root.sans; font.pixelSize: root.px(13); Layout.preferredWidth: 120; Layout.alignment: Qt.AlignTop }
        Text { text: parent.value; color: parent.tone; font.family: parent.monospace ? root.mono : root.sans; font.pixelSize: root.px(13); wrapMode: Text.WrapAnywhere; Layout.fillWidth: true }
    }

    // ── Screen ──────────────────────────────────────────────────────────
    Rectangle { anchors.fill: parent; color: root.slate }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 14

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Text { text: "Program Registry"; color: root.ink; font.family: root.sans; font.pixelSize: root.px(24); font.weight: Font.Bold }
            Text {
                Layout.fillWidth: true
                text: root.config && root.config.registry ? "registry " + Logic.shortId(root.config.registry) + " on " + root.config.sequencer : ""
                color: root.dim; font.family: root.sans; font.pixelSize: root.px(13); elide: Text.ElideRight
            }
            Action { label: root.settingsOpen ? "Close settings" : "Settings"; onPressed: root.settingsOpen = !root.settingsOpen }
        }

        // Settings: which sequencer and registry the module reads.
        Rectangle {
            visible: root.settingsOpen
            Layout.fillWidth: true
            implicitHeight: settingsCol.implicitHeight + 24
            radius: 8; color: root.card; border.color: root.line
            ColumnLayout {
                id: settingsCol
                anchors.fill: parent; anchors.margins: 12; spacing: 8
                Text { text: "Sequencer (https://, or http:// on this machine)"; color: root.dim; font.family: root.sans; font.pixelSize: root.px(13) }
                Field { id: sequencerField; Layout.fillWidth: true; placeholder: "http://127.0.0.1:3062"; monospace: true }
                Text { text: "Registry program id"; color: root.dim; font.family: root.sans; font.pixelSize: root.px(13) }
                Field { id: registryField; Layout.fillWidth: true; placeholder: "base58 program id"; monospace: true }
                Action { label: "Save"; strong: true; onPressed: root.saveConfig() }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Field { id: searchField; Layout.fillWidth: true; placeholder: "Search by name"; onAccepted: { root.query = text.trim(); root.loadList(false) } }
            Action { label: "Search"; strong: true; onPressed: { root.query = searchField.text.trim(); root.loadList(false) } }
            Action { label: "All"; visible: root.query !== ""; onPressed: { searchField.text = ""; root.query = ""; root.loadList(false) } }
        }

        // Requests from other apps.
        Repeater {
            model: root.requests
            Rectangle {
                required property var modelData
                Layout.fillWidth: true
                implicitHeight: reqCol.implicitHeight + 20
                radius: 8; color: Qt.rgba(root.brass.r, root.brass.g, root.brass.b, 0.12); border.color: root.brass
                ColumnLayout {
                    id: reqCol
                    anchors.fill: parent; anchors.margins: 10; spacing: 4
                    Text {
                        Layout.fillWidth: true
                        text: Logic.requestTitle(modelData)
                        color: root.ink; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(14); font.weight: Font.DemiBold
                    }
                    Repeater {
                        model: Logic.changeLines(modelData.change)
                        Text { required property var modelData; Layout.fillWidth: true; text: modelData; color: root.dim; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(13) }
                    }
                    RowLayout {
                        spacing: 8
                        Action { label: "Approve and sign"; strong: true; grave: !!(modelData.change && modelData.change.summary && modelData.change.summary.irreversible); onPressed: root.answerRequest(modelData.request, true) }
                        Action { label: "Refuse"; onPressed: root.answerRequest(modelData.request, false) }
                    }
                }
            }
        }

        Text {
            visible: root.notice !== ""
            Layout.fillWidth: true
            text: root.notice; color: root.brass; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(13)
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 16

            // The list.
            Rectangle {
                Layout.preferredWidth: 420
                Layout.fillHeight: true
                radius: 8; color: root.card; border.color: root.line
                ColumnLayout {
                    anchors.fill: parent; anchors.margins: 10; spacing: 6
                    Text {
                        text: root.loading ? "Reading the registry…" : root.problem !== "" ? root.problem
                              : (root.query !== "" ? root.items.length + " matching “" + root.query + "”" : root.items.length + " of " + root.total + " entries, newest first")
                        color: root.problem !== "" ? root.alarm : root.dim
                        wrapMode: Text.Wrap; Layout.fillWidth: true
                        font.family: root.sans; font.pixelSize: root.px(13)
                    }
                    ListView {
                        id: list
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        spacing: 4
                        model: root.items
                        delegate: Rectangle {
                            required property var modelData
                            readonly property string kind: Logic.kindOf(modelData)
                            width: list.width
                            height: 58
                            radius: 6
                            color: root.selected && root.selected.address === modelData.address ? root.raised : rowArea.containsMouse ? Qt.rgba(root.raised.r, root.raised.g, root.raised.b, 0.6) : "transparent"
                            ColumnLayout {
                                anchors.fill: parent; anchors.margins: 8; spacing: 2
                                RowLayout {
                                    spacing: 8
                                    Text {
                                        text: modelData.entry ? modelData.entry.name + "  v" + modelData.entry.version : modelData.link.name
                                        color: root.ink; font.family: root.sans; font.pixelSize: root.px(15); font.weight: Font.DemiBold
                                    }
                                    KindBadge { kind: parent.parent.parent.kind }
                                    Text { visible: !!(modelData.entry && modelData.entry.frozen); text: "frozen"; color: root.dim; font.pixelSize: root.px(12); font.family: root.sans }
                                }
                                Text {
                                    text: "program " + Logic.shortId(modelData.link.program) + "  ·  #" + modelData.seq
                                    color: root.dim; font.family: root.mono; font.pixelSize: root.px(12)
                                }
                            }
                            MouseArea { id: rowArea; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: root.open(modelData) }
                        }
                    }
                    Action { visible: root.next !== "" && root.query === ""; label: "More"; onPressed: root.loadList(true) }
                }
            }

            // The entry.
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                radius: 8; color: root.card; border.color: root.line
                Text {
                    visible: !root.selected
                    anchors.centerIn: parent
                    text: "Pick an entry to see what the registry says about it."
                    color: root.dim; font.family: root.sans; font.pixelSize: root.px(14)
                }
                Flickable {
                    visible: !!root.selected
                    anchors.fill: parent
                    anchors.margins: 14
                    clip: true
                    contentHeight: detail.implicitHeight
                    ColumnLayout {
                        id: detail
                        width: parent.width
                        spacing: 8
                        readonly property var e: root.selected ? root.selected.entry : null
                        readonly property string kind: root.selected ? Logic.kindOf(root.selected) : ""

                        RowLayout {
                            spacing: 10
                            Text { text: detail.e ? detail.e.name : ""; color: root.ink; font.family: root.sans; font.pixelSize: root.px(22); font.weight: Font.Bold }
                            Text { text: detail.e ? "v" + detail.e.version : ""; color: root.dim; font.family: root.sans; font.pixelSize: root.px(18) }
                            KindBadge { kind: detail.kind }
                            Text {
                                visible: !!(root.label && root.label.verified)
                                text: "✓ verified here"
                                color: root.good; font.family: root.sans; font.pixelSize: root.px(13); font.weight: Font.DemiBold
                            }
                        }
                        Text {
                            Layout.fillWidth: true
                            text: Logic.kindBadge(detail.kind).note
                            color: root.dim; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(12)
                        }
                        Text {
                            Layout.fillWidth: true
                            visible: !!(detail.e && detail.e.description)
                            text: detail.e ? detail.e.description : ""
                            color: root.ink; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(14)
                        }
                        // Is the entry still about the code that runs?
                        Text {
                            readonly property var f: Logic.freshnessText(root.resolved ? root.resolved.freshness : null)
                            visible: f.text !== ""
                            text: f.text
                            color: f.tone === "good" ? root.good : f.tone === "warn" ? root.brass : root.dim
                            font.family: root.sans; font.pixelSize: root.px(13); font.weight: Font.DemiBold
                        }
                        Row2 { key: "Program"; value: detail.e ? detail.e.program : ""; monospace: true }
                        Row2 { key: "Entry"; value: root.selected ? root.selected.address : ""; monospace: true }
                        Row2 { key: "Author (proven)"; value: detail.e ? detail.e.author : ""; monospace: true }
                        Row2 { key: "Author label"; visible: !!(detail.e && detail.e.author_label); value: detail.e ? detail.e.author_label + "  (unproven text)" : ""; tone: root.dim }
                        Row2 { key: "Tags"; visible: !!(detail.e && detail.e.tags.length); value: detail.e ? detail.e.tags.join(", ") : "" }
                        Row2 { key: "Revision"; value: detail.e ? detail.e.revision + (detail.e.frozen ? "  (frozen)" : "") + (detail.e.born_with_deploy ? "  · born with its deploy" : "") : "" }
                        Row2 { key: "Registered"; value: detail.e ? Logic.when(detail.e.registered_at) : "" }
                        Row2 { key: "Updated"; value: detail.e ? Logic.when(detail.e.updated_at) : "" }
                        Row2 {
                            key: "Code"
                            value: root.resolved && root.resolved.header ? (root.resolved.header.immutable ? "immutable" : "upgradeable by its header key") + ", image " + root.resolved.header.image_id.slice(0, 16) + "…" : "no program deployed"
                            monospace: false
                        }
                        Row2 { key: "Source"; value: detail.e ? (detail.e.source.repo || "archive on Storage") + (detail.e.source.commit ? " @ " + detail.e.source.commit.slice(0, 12) : "") : ""; monospace: true }
                        Row2 {
                            key: "Verified here"
                            value: root.label ? (root.label.verified ? "yes: this machine rebuilt the running code from its source" : "no (run `lezreg verify " + Logic.shortId(detail.e ? detail.e.program : "") + " --mark`)") : "…"
                            tone: root.label && root.label.verified ? root.good : root.dim
                        }
                        Row2 {
                            key: "Community"
                            visible: detail.kind === "deployer"
                            value: root.community.length ? root.community.length + " community entr" + (root.community.length === 1 ? "y" : "ies") + " for this program" : "none"
                            tone: root.dim
                        }

                        // The IDL, or why it can't be shown.
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: idlCol.implicitHeight + 16
                            radius: 6; color: root.slate; border.color: root.line
                            readonly property var s: Logic.idlSummary(root.idl)
                            ColumnLayout {
                                id: idlCol
                                anchors.fill: parent; anchors.margins: 8; spacing: 4
                                Text {
                                    Layout.fillWidth: true
                                    text: root.idl ? parent.parent.s.text : "Fetching the IDL from Logos Storage…"
                                    color: parent.parent.s.ok ? root.ink : root.dim
                                    wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(13)
                                }
                                Text {
                                    Layout.fillWidth: true
                                    visible: parent.parent.s.instructions.length > 0
                                    text: parent.parent.s.instructions.join("  ·  ")
                                    color: root.dim; wrapMode: Text.Wrap; font.family: root.mono; font.pixelSize: root.px(12)
                                }
                            }
                        }

                        // Author tools: prepared by the module, signed by your lezreg wallet.
                        Text { text: "If your lezreg wallet is this entry's author"; color: root.ink; font.family: root.sans; font.pixelSize: root.px(15); font.weight: Font.DemiBold; Layout.topMargin: 8 }
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            Field { id: versionField; Layout.preferredWidth: 120; text: root.editVersion; placeholder: "version" }
                            Field { id: descField; Layout.fillWidth: true; text: root.editDescription; placeholder: "description" }
                            Action {
                                label: "Update…"
                                enabledNow: !!detail.e && !detail.e.frozen
                                onPressed: root.prepare("prepare_update", { version: versionField.text.trim(), description: descField.text })
                            }
                            Action { label: "Freeze…"; grave: true; enabledNow: !!detail.e && !detail.e.frozen; onPressed: root.prepare("prepare_freeze", {}) }
                        }

                        // The confirm sheet.
                        Rectangle {
                            visible: !!root.change
                            Layout.fillWidth: true
                            implicitHeight: sheet.implicitHeight + 20
                            radius: 8; color: Qt.rgba(root.brass.r, root.brass.g, root.brass.b, 0.12); border.color: root.brass
                            ColumnLayout {
                                id: sheet
                                anchors.fill: parent; anchors.margins: 10; spacing: 4
                                Repeater {
                                    model: Logic.changeLines(root.change)
                                    Text { required property var modelData; Layout.fillWidth: true; text: modelData; color: root.ink; wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(13) }
                                }
                                RowLayout {
                                    spacing: 8
                                    Action { label: "Sign and send"; strong: true; grave: !!(root.change && root.change.summary && root.change.summary.irreversible); onPressed: root.submitChange() }
                                    Action { label: "Cancel"; onPressed: root.change = null }
                                }
                            }
                        }
                        Text {
                            visible: !!root.job
                            Layout.fillWidth: true
                            text: Logic.jobText(root.job)
                            color: root.job && root.job.state === "failed" ? root.alarm : root.job && root.job.state === "applied" ? root.good : root.dim
                            wrapMode: Text.Wrap; font.family: root.sans; font.pixelSize: root.px(13)
                        }
                        // The reusable label, as another module would show this program.
                        RowLayout {
                            spacing: 8
                            Layout.topMargin: 8
                            Text { text: "As other apps show it:"; color: root.dim; font.family: root.sans; font.pixelSize: root.px(12) }
                            RegistryLabel {
                                bridge: root.bridgeReady ? logos : null
                                programId: detail.e ? detail.e.program : ""
                                pixelSize: root.px(13)
                                fontFamily: root.sans
                                textColor: root.ink
                                quietColor: root.dim
                                verifiedColor: root.good
                                warnColor: root.brass
                            }
                        }
                    }
                }
            }
        }
    }
}
