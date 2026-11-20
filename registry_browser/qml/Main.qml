// Program Registry for Basecamp: an index of the programs on this network, read like a
// guidebook. Each entry has a title, one plain sentence and its publisher; open one for its guide
// page (what it is, who published it, its source, what you can do), with everything technical
// (ids, revisions, code, IDL, this machine's verify mark) folded under "Technical details". An
// entry your lezreg wallet authors can be updated or frozen from there.
//
// A front end on the `program_registry` core module, which reads the sequencer itself; nothing
// here talks to the network. Changes are prepared by the module (`prepare_*`), shown on a
// confirm sheet, and signed by your own `lezreg` CLI wallet (`submit`).
//
// The parts in kit/ (index row, section heading, guide header, technical details, buttons, page
// header) hold no registry logic: they are the shared look of our apps (Names Wallet is the
// reference). Everything about entries is in logic.mjs, tested by logic.test.mjs.

import QtQuick
import QtQuick.Layouts
import Logos.Theme
import "logic.mjs" as Logic
import "kit"

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
    property var selected: null        // the listed item open on the guide page
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
    property bool remoteOk: false      // the person ticked "this spends real funds on <sequencer>"
    property bool busy: false          // a change is being prepared or sent
    property string workingOn: ""      // which: a prepare_* method or "submit"
    property var accountNames: ({})    // account → LNS name, from the Names app's lookup
    property string guideNote: ""      // "Program ID copied." and such, under the guide's buttons
    // Sign / Approve buttons rest for a moment after a card appears or changes, so a click meant
    // for something else can't land on them as the layout shifts.
    property bool armed: false
    readonly property bool remote: !!(config && config.sequencer && config.sequencer.indexOf("https://") === 0)
    onChangeChanged: { armed = false; armTimer.restart() }
    onRequestsChanged: { armed = false; armTimer.restart() }
    Timer { id: armTimer; interval: 1500; onTriggered: root.armed = true }
    property string notice: ""

    readonly property bool bridgeReady: typeof logos !== "undefined" && !!logos && !!logos.callModuleAsync

    // What the screen shows, derived from the entries (logic.mjs).
    readonly property var rows: Logic.indexRows(items)
    readonly property var guide: Logic.guideOf(selected, items)
    readonly property var entry: selected ? selected.entry : null
    readonly property string kind: selected ? Logic.kindOf(selected) : ""
    readonly property var source: entry ? Logic.sourceLink(entry.source) : null

    // ── Look (kit/Look.qml: the palette and type every app shares) ──────
    property Look look: Look {}
    // One left edge for everything; the index at 300 px, the guide page beside it in a column of
    // at most 680. Too narrow for both: the guide page replaces the index, with Back.
    readonly property bool narrow: width < look.edge * 2 + look.indexWidth + 32 + 420

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
            if (v.registry) loadList(false)
            else { settingsOpen = true; problem = "No registry is set: see Network settings." }
        }, function(e) { problem = e })
    }

    function loadList(more) {
        loading = true
        problem = ""
        if (query !== "") {
            // Names are "names-evm-bridge"; people type titles ("Names EVM Bridge"): try each form.
            var forms = Logic.searchForms(query)
            var asked = query
            var tryForm = function(n) {
                if (n >= forms.length) { items = []; next = ""; total = 0; loading = false; return }
                ask("search_name", [forms[n], 50], function(v) {
                    if (root.query !== asked) return    // a newer search took over
                    if (v.items.length === 0 && n + 1 < forms.length) { tryForm(n + 1); return }
                    items = v.items; next = ""; total = v.items.length; loading = false
                    if (v.truncated) notice = "The search looked at the newest " + v.scanned + " entries."
                }, function(e) { problem = e; loading = false })
            }
            tryForm(0)
            return
        }
        ask("list_entries", [more ? next : "", 25], function(v) {
            items = more ? items.concat(v.items) : v.items
            next = v.next || ""
            total = v.total
            loading = false
        }, function(e) { problem = e; loading = false })
    }

    function open(item, keepDetails) {
        selected = item
        resolved = null; idl = null; label = null; community = []
        change = null
        guideNote = ""
        if (!keepDetails) { techDetails.open = false; authorTools.open = false }
        guideFlick.contentY = 0
        var program = item.entry ? item.entry.program : item.link.program
        editVersion = item.entry ? item.entry.version : ""
        editDescription = item.entry ? item.entry.description : ""
        ask("resolve_program", [program], function(v) { resolved = v }, function(e) { notice = e })
        ask("program_label", [program], function(v) { label = v }, function(e) {})
        ask("third_party_entries", [program], function(v) { community = v }, function(e) {})
        ask("get_idl", [program], function(v) { idl = v }, function(e) { idl = { status: "unavailable: " + e, idl: null } }, 30000)
        lookUpName(item.entry ? item.entry.author : item.link.registrant)
    }

    function openAddress(address) {
        for (var i = 0; i < items.length; i++)
            if (items[i].address === address) { open(items[i]); return }
    }

    /// The LNS name an account goes by, from the Names app's own lookup (`names.reverse`), shown
    /// beside the account in Technical details. No Names app, no name, or no answer: nothing shown.
    function lookUpName(account) {
        if (!account || !bridgeReady || accountNames[account] !== undefined) return
        var pending = Object.assign({}, accountNames); pending[account] = ""; accountNames = pending
        logos.callModuleAsync("names", "reverse", [account], function(reply) {
            var name = Logic.reverseName(reply)
            if (name === "") return
            var known = Object.assign({}, root.accountNames); known[account] = name; root.accountNames = known
        }, 8000)
    }

    function accountLine(account) {
        var name = accountNames[account] || ""
        return account + (name !== "" ? "\nGoes by " + name + " (LNS, from the Names app)" : "")
    }

    function prepare(method, fields) {
        notice = ""
        var input = Object.assign({ entry: selected.address }, fields)
        // A sequencer that is not on this machine spends real funds: only when the person said so.
        if (remote) {
            if (!remoteOk) {
                notice = "This sequencer (" + config.sequencer + ") is not on this machine: changes there spend real funds. Tick the box first."
                return
            }
            input.allow_remote = true
        }
        busy = true; workingOn = method
        ask(method, [JSON.stringify(input)], function(v) { busy = false; workingOn = ""; change = v },
            function(e, code) { busy = false; workingOn = ""; notice = e + (code ? " (" + code + ")" : "") })
    }

    function submitChange() {
        var c = change
        change = null
        remoteOk = false   // "spends real funds" is asked again for every change
        busy = true; workingOn = "submit"
        ask("submit", [JSON.stringify(c)], function(v) { busy = false; workingOn = ""; job = v; pollJob() },
            function(e, code) { busy = false; workingOn = ""; notice = e + (code ? " (" + code + ")" : "") })
    }

    function pollJob() {
        if (!job || job.state !== "running") return
        ask("submit_status", [job.job], function(v) {
            job = v
            if (v.state === "running") jobTimer.start()
            else if (v.state === "applied" && selected) open(selected, true)
        }, function(e) { notice = e })
    }

    Timer { id: jobTimer; interval: 2000; onTriggered: root.pollJob() }

    // Other apps can't sign through the registry: their changes wait here for the person.
    function loadRequests() {
        ask("pending_requests", [], function(v) {
            // Oldest first; and only a real change re-arms the buttons (a poll that returns the same
            // list must not disable Approve under the cursor).
            var sorted = (v || []).slice().sort(function(a, b) { return a.at - b.at })
            if (JSON.stringify(sorted) !== JSON.stringify(requests)) requests = sorted
        }, function(e) {})
    }
    function answerRequest(id, approve) {
        ask(approve ? "approve_request" : "deny_request", [id], function(v) {
            if (approve) { job = v; pollJob() }
            loadRequests()
        }, function(e, code) { notice = e + (code ? " (" + code + ")" : ""); loadRequests() })
    }
    Timer { interval: 5000; running: root.bridgeReady; repeat: true; triggeredOnStart: true; onTriggered: root.loadRequests() }

    Component.onCompleted: loadConfig()

    // ── Guide page actions ──────────────────────────────────────────────
    function copy(text, what) {
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        guideNote = what + " copied."
    }

    /// Opens the entry's source in the browser: only an https address (sourceLink lets no other
    /// kind through), and only if Basecamp lets this app open one.
    function viewSource() {
        if (!source) return
        var opened = false
        try { opened = Qt.openUrlExternally(source.url) } catch (error) { opened = false }
        guideNote = opened ? "Opened " + source.url : "Basecamp did not open the link. Copy it instead: " + source.url
    }

    function guideAction(key) {
        if (key === "source") viewSource()
        else if (key === "copy-id" && entry) copy(entry.program, "Program ID")
        else if (key === "copy-source" && source) copy(source.url, "Source link")
    }

    TextEdit { id: clipboard; visible: false }

    // ── Screen ──────────────────────────────────────────────────────────
    Rectangle { anchors.fill: parent; color: look.slate }

    ColumnLayout {
        id: screen
        anchors { fill: parent; leftMargin: root.width >= 600 ? look.edge : 24; rightMargin: root.narrow ? (root.width >= 600 ? look.edge : 24) : 32; topMargin: 40 }
        spacing: 16
        readonly property real columnWidth: root.narrow ? Math.min(width, look.pageWidth) : Math.min(width, look.indexWidth + 40 + look.pageWidth)

        // The app's header, the network it reads, and its settings: hidden while a guide page
        // stands in for the index in a narrow window.
        RowLayout {
            visible: !(root.narrow && root.selected)
            Layout.preferredWidth: screen.columnWidth
            Layout.maximumWidth: screen.columnWidth
            spacing: 16
            PageHeader {
                look: root.look
                title: "Program Registry"
                sentence: "What each program on this network does, who published it, and where its source is."
                runsOnPrefix: "Reads"
                network: root.config ? Logic.clean(root.config.network && root.config.network.name ? root.config.network.name : Logic.hostOf(root.config.sequencer)) : ""
                networkDetail: root.config && root.config.network && root.config.network.name ? Logic.hostOf(root.config.sequencer) : ""
            }
            Action {
                look: root.look
                Layout.alignment: Qt.AlignTop
                label: root.settingsOpen ? "Hide network settings" : "Show network settings"
                onPressed: root.settingsOpen = !root.settingsOpen
            }
        }

        // Settings: which sequencer and registry the module reads.
        Rectangle {
            visible: root.settingsOpen && !(root.narrow && root.selected)
            Layout.preferredWidth: screen.columnWidth
            Layout.maximumWidth: screen.columnWidth
            implicitHeight: settingsCol.implicitHeight + 32
            radius: 8; color: look.card; border.color: look.line
            ColumnLayout {
                id: settingsCol
                anchors { left: parent.left; right: parent.right; top: parent.top; margins: 16 }
                spacing: 14
                FactRow { look: root.look; key: "Sequencer"; value: Logic.clean(root.config ? root.config.sequencer : ""); monospace: true }
                FactRow { look: root.look; key: "Registry"; value: Logic.clean(root.config && root.config.registry ? root.config.registry : "(none)"); monospace: true }
                FactRow {
                    look: root.look
                    visible: !!(root.config && root.config.network && root.config.network.chain_id)
                    key: "Network profile"
                    value: root.config && root.config.network ? Logic.clean(root.config.network.name || "") + " · chain " + Logic.shortId(root.config.network.chain_id) + " · " + Logic.clean(root.config.network.source || "") : ""
                }
                FactRow { look: root.look; visible: !!(root.config && root.config.network && root.config.network.error); key: "Not used"; tone: look.alarm
                          value: root.config && root.config.network && root.config.network.error ? "Network file: " + Logic.clean(root.config.network.error) : "" }
                Text { textFormat: Text.PlainText; Layout.fillWidth: true; wrapMode: Text.Wrap; color: look.dim; font.family: look.sans; font.pixelSize: look.px(14)
                       text: "The sequencer and the registry decide what every app is told and where changes are signed, so only the host sets them: logosctl call program_registry config_set (sequencer, registry, or network_file), or ~/.config/lez-registry/module.json. Out of the box they come from the module's built-in network profile." }
            }
        }

        // Requests from other apps: in their own scroll area, so long ones can't push the
        // browser (and its buttons) off the screen.
        Flickable {
            id: reqFlick
            visible: root.requests.length > 0
            Layout.preferredWidth: screen.columnWidth
            Layout.maximumWidth: screen.columnWidth
            Layout.preferredHeight: Math.min(reqColumn.implicitHeight, 280)
            clip: true
            contentWidth: width
            contentHeight: reqColumn.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            ColumnLayout {
                id: reqColumn
                width: reqFlick.width
                spacing: 8
                Repeater {
                    model: root.requests
                    Rectangle {
                        required property var modelData
                        Layout.fillWidth: true
                        implicitHeight: reqCol.implicitHeight + 32
                        radius: 8; color: look.card; border.color: look.brass
                        ColumnLayout {
                            id: reqCol
                            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 16 }
                            spacing: 6
                            Text { textFormat: Text.PlainText;
                                Layout.fillWidth: true
                                text: Logic.requestTitle(modelData)
                                color: look.ink; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(16); font.weight: Font.DemiBold
                            }
                            Repeater {
                                model: Logic.changeLines(modelData.change)
                                Text { textFormat: Text.PlainText; required property var modelData; Layout.fillWidth: true; text: modelData; color: look.dim; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(14) }
                            }
                            RowLayout {
                                spacing: 8
                                Layout.topMargin: 8
                                Action { look: root.look; label: "Approve and sign change"; enabledNow: root.armed; strong: true; grave: !!(modelData.change && modelData.change.summary && modelData.change.summary.irreversible); onPressed: root.answerRequest(modelData.request, true) }
                                Action { look: root.look; label: "Reject request"; onPressed: root.answerRequest(modelData.request, false) }
                            }
                        }
                    }
                }
            }
        }

        Text { textFormat: Text.PlainText;
            visible: root.notice !== ""
            Layout.preferredWidth: screen.columnWidth
            Layout.maximumWidth: screen.columnWidth
            text: Logic.clean(root.notice); color: look.alarm; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(16)
        }
        // The last change sent (yours, or another app's you approved): sending, applied or failed.
        Text { textFormat: Text.PlainText;
            visible: !!root.job
            Layout.preferredWidth: screen.columnWidth
            Layout.maximumWidth: screen.columnWidth
            text: Logic.jobText(root.job)
            color: root.job && root.job.state === "failed" ? look.alarm : root.job && root.job.state === "applied" ? look.good : look.dim
            wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(16)
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 40

            // ── The index ───────────────────────────────────────────────
            ColumnLayout {
                visible: !(root.narrow && root.selected)
                Layout.preferredWidth: root.narrow ? screen.columnWidth : look.indexWidth
                Layout.maximumWidth: root.narrow ? screen.columnWidth : look.indexWidth
                Layout.fillHeight: true
                spacing: 8
                Field {
                    id: searchField
                    look: root.look
                    Layout.fillWidth: true
                    hint: "Search programs by name"
                    glyph: "search"
                    onAccepted: { root.query = text.trim(); root.loadList(false) }
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12
                    Text { textFormat: Text.PlainText;
                        Layout.fillWidth: true
                        text: root.loading ? "Reading the registry…" : root.problem !== "" ? Logic.clean(root.problem)
                              : root.query !== "" ? root.items.length + " matching “" + Logic.clean(root.query) + "”"
                              : root.items.length < root.total ? root.items.length + " of " + root.total + " programs" : root.total + " program" + (root.total === 1 ? "" : "s")
                        color: root.problem !== "" ? look.alarm : look.dim
                        wrapMode: Text.Wrap
                        font.family: look.sans; font.pixelSize: look.px(14)
                    }
                    Link { look: root.look; visible: root.query !== ""; text: "Show all programs"; font.pixelSize: look.px(14); onPressed: { searchField.text = ""; root.query = ""; root.loadList(false) } }
                }
                ListView {
                    id: list
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    spacing: 2
                    model: root.rows
                    boundsBehavior: Flickable.StopAtBounds
                    delegate: Item {
                        required property var modelData
                        required property int index
                        readonly property bool isHeading: modelData.type === "heading"
                        width: list.width
                        height: isHeading ? heading.implicitHeight + (index > 0 ? 24 : 14) : indexRow.implicitHeight
                        SectionHeading {
                            id: heading
                            look: root.look
                            visible: parent.isHeading
                            anchors { left: parent.left; right: parent.right; bottom: parent.bottom; bottomMargin: 6 }
                            text: parent.isHeading ? modelData.text : ""
                        }
                        IndexRow {
                            id: indexRow
                            look: root.look
                            visible: !parent.isHeading
                            width: parent.width
                            title: parent.isHeading ? "" : modelData.title
                            sentence: parent.isHeading ? "" : modelData.sentence
                            publisher: parent.isHeading ? "" : modelData.publisher
                            proven: parent.isHeading || modelData.proven
                            note: parent.isHeading ? "" : modelData.note
                            secondary: !parent.isHeading && modelData.secondary
                            selected: !parent.isHeading && !!root.selected && root.selected.address === modelData.item.address
                            onPressed: root.open(modelData.item)
                        }
                    }
                }
                Text { textFormat: Text.PlainText;
                    visible: root.rows.some(function(r) { return r.type === "entry" && !r.proven })
                    Layout.fillWidth: true
                    text: "Publishers in italics named themselves; nothing on the chain confirms those names."
                    color: look.dim; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(13); font.italic: true
                }
                Action { look: root.look; visible: root.next !== "" && root.query === ""; label: "Show more programs"; busyLabel: "Reading…"; working: root.loading; onPressed: root.loadList(true) }
                Item { Layout.preferredHeight: 16 }
            }

            // ── The guide page ──────────────────────────────────────────
            Flickable {
                id: guideFlick
                visible: !root.narrow || !!root.selected
                Layout.fillWidth: true
                Layout.maximumWidth: look.pageWidth
                Layout.fillHeight: true
                clip: true
                contentWidth: width
                contentHeight: page.implicitHeight + 48
                boundsBehavior: Flickable.StopAtBounds

                Text { textFormat: Text.PlainText;
                    visible: !root.selected
                    width: parent.width
                    y: 56
                    text: root.loading || root.items.length === 0 ? "" : "Pick a program to read what it does, who published it, and where its source is."
                    color: look.dim; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(17)
                }

                ColumnLayout {
                    id: page
                    visible: !!root.selected
                    width: guideFlick.width
                    spacing: 16

                    GuideHeader {
                        look: root.look
                        backLabel: root.narrow ? "Programs" : ""
                        onBack: root.selected = null
                        section: root.guide ? root.guide.section : ""
                        title: root.guide ? root.guide.title : ""
                        sentence: root.guide ? root.guide.sentence : ""
                        publisher: root.guide ? root.guide.publisher : ""
                        proven: !root.guide || root.guide.proven
                        publisherNote: root.guide && !root.guide.proven ? "a name they gave themselves" : ""
                        // What a reader must know before trusting it: the program no longer runs
                        // the code this entry describes, or nothing runs there at all.
                        warning: {
                            var f = Logic.freshnessText(root.resolved ? root.resolved.freshness : null)
                            if (f.tone === "warn") return f.text
                            if (root.label && root.label.stale) return "Stale: the program now runs other code than this entry describes"
                            if (root.label && root.label.deployed === false) return "No program is deployed at this id"
                            return ""
                        }
                        actions: {
                            var a = []
                            if (root.source) a.push({ key: "source", label: "View source", strong: true })
                            if (root.entry) a.push({ key: "copy-id", label: "Copy program ID" })
                            if (root.source) a.push({ key: "copy-source", label: "Copy source link" })
                            return a
                        }
                        onActionPressed: function(key) { root.guideAction(key) }
                    }
                    Text { textFormat: Text.PlainText;
                        visible: root.guideNote !== ""
                        Layout.fillWidth: true
                        text: Logic.clean(root.guideNote)
                        color: look.dim; wrapMode: Text.WrapAnywhere; font.family: look.sans; font.pixelSize: look.px(15)
                    }

                    // About: the publisher's whole description, when it says more than the sentence.
                    ColumnLayout {
                        visible: !!(root.guide && root.guide.about !== "" && root.guide.about !== root.guide.sentence)
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        spacing: 8
                        SectionHeading { look: root.look; Layout.fillWidth: true; text: "About" }
                        Text { textFormat: Text.PlainText;
                            Layout.fillWidth: true
                            text: root.guide ? root.guide.about : ""
                            color: look.ink; wrapMode: Text.Wrap; font.family: look.sans; font.weight: Font.Medium; font.pixelSize: look.px(16); lineHeight: 1.3
                        }
                    }

                    // At a glance, in plain words.
                    ColumnLayout {
                        visible: !!root.entry
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        spacing: 10
                        SectionHeading { look: root.look; Layout.fillWidth: true; text: "At a glance" }
                        FactRow { look: root.look; key: "Version"; value: root.entry ? Logic.clean(root.entry.version) + (root.entry.frozen ? " (frozen: it never changes again)" : "") : "" }
                        FactRow { look: root.look; key: "Source"; value: root.source ? root.source.text + (root.entry.source.commit ? " at " + Logic.clean(root.entry.source.commit).slice(0, 7) : "") : "an archive on Logos Storage (see Technical details)" }
                        FactRow { look: root.look; key: "Listed"; value: root.entry ? Logic.day(root.entry.registered_at) + (root.entry.updated_at && Logic.day(root.entry.updated_at) !== Logic.day(root.entry.registered_at) ? ", updated " + Logic.day(root.entry.updated_at) : "") : "" }
                        FactRow { look: root.look; key: "Listing"; value: Logic.listedBy(root.kind) }
                    }

                    // The rest of its family, newest first.
                    ColumnLayout {
                        visible: !!(root.guide && root.guide.family.length)
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        spacing: 10
                        SectionHeading { look: root.look; Layout.fillWidth: true; text: "Other versions" }
                        Flow {
                            Layout.fillWidth: true
                            spacing: 20
                            Repeater {
                                model: root.guide ? root.guide.family : []
                                Link { required property var modelData; look: root.look; text: modelData.label; onPressed: root.openAddress(modelData.address) }
                            }
                        }
                    }

                    // ── Technical details: everything the registry holds, folded ──
                    TechDetails {
                        id: techDetails
                        look: root.look
                        Layout.topMargin: 8
                        summary: root.entry ? "Program " + Logic.shortId(root.entry.program) + " · revision " + root.entry.revision
                                 + (root.resolved && root.resolved.freshness && root.resolved.freshness.status === "fresh" ? " · runs the registered code" : "") : ""

                        Text { textFormat: Text.PlainText;
                            readonly property var f: Logic.freshnessText(root.resolved ? root.resolved.freshness : null)
                            visible: f.text !== ""
                            Layout.fillWidth: true
                            text: f.text
                            color: f.tone === "good" ? look.good : f.tone === "warn" ? look.alarm : look.dim
                            wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(16); font.weight: Font.DemiBold
                        }
                        FactRow { look: root.look; key: "Program"; value: root.entry ? root.entry.program : ""; monospace: true }
                        FactRow { look: root.look; key: "Entry"; value: root.selected ? root.selected.address : ""; monospace: true }
                        FactRow { look: root.look; key: "Author (proven)"; value: root.entry ? root.accountLine(root.entry.author) : ""; monospace: true }
                        FactRow { look: root.look; key: "Author label"; visible: !!(root.entry && root.entry.author_label); tone: look.dim
                                  value: root.entry ? Logic.clean(root.entry.author_label) + (root.entry.author_label_proven ? "" : "  (their own words, not checked)") : "" }
                        FactRow { look: root.look; key: "Kind"; value: Logic.kindBadge(root.kind).text + ": " + Logic.kindBadge(root.kind).note; tone: look.dim }
                        FactRow { look: root.look; key: "Name, version"; value: root.entry ? Logic.clean(root.entry.name) + " " + Logic.clean(root.entry.version) : "" }
                        FactRow { look: root.look; key: "Tags"; visible: !!(root.entry && root.entry.tags.length); value: root.entry ? root.entry.tags.map(Logic.clean).join(", ") : "" }
                        FactRow { look: root.look; key: "Revision"; value: root.entry ? root.entry.revision + (root.entry.frozen ? "  (frozen)" : "") + (root.entry.born_with_deploy ? "  · born with its deploy" : "") + "  · #" + root.selected.seq + " in the registry" : "" }
                        FactRow { look: root.look; key: "Registered"; value: root.entry ? Logic.when(root.entry.registered_at) : "" }
                        FactRow { look: root.look; key: "Updated"; value: root.entry ? Logic.when(root.entry.updated_at) : "" }
                        FactRow {
                            look: root.look
                            key: "Code"
                            value: root.resolved && root.resolved.header ? (root.resolved.header.immutable ? "immutable" : "upgradeable by its header key") + ", image " + root.resolved.header.image_id.slice(0, 16) + "…" : root.resolved ? "no program deployed" : "…"
                        }
                        FactRow { look: root.look; key: "Source"; value: root.entry ? Logic.clean(root.entry.source.repo || "archive on Storage") + (root.entry.source.commit ? " @ " + Logic.clean(root.entry.source.commit) : "") : ""; monospace: true }
                        FactRow { look: root.look; key: "Build"; visible: !!(root.entry && root.entry.build); value: root.entry && root.entry.build ? Logic.clean(root.entry.build.binary) + " from " + Logic.clean(root.entry.build.manifest_path) + ", builder " + Logic.clean(root.entry.build.image_tag) + ", risc0-build " + Logic.clean(root.entry.build.risc0_build) : "" }
                        FactRow {
                            look: root.look
                            key: "Verified here"
                            value: root.label ? (root.label.verified ? "yes: this machine rebuilt the running code from its source" : "no (run `lezreg verify " + Logic.shortId(root.entry ? root.entry.program : "") + " --mark`)") : "…"
                            tone: root.label && root.label.verified ? look.good : look.dim
                        }
                        FactRow {
                            look: root.look
                            key: "Community"
                            visible: root.kind === "deployer"
                            value: root.community.length ? root.community.length + " community entr" + (root.community.length === 1 ? "y" : "ies") + " for this program" : "none"
                            tone: look.dim
                        }

                        // The IDL, or why it can't be shown.
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: idlCol.implicitHeight + 28
                            radius: 6; color: look.slate; border.color: look.line
                            readonly property var s: Logic.idlSummary(root.idl)
                            ColumnLayout {
                                id: idlCol
                                anchors { left: parent.left; right: parent.right; top: parent.top; margins: 14 }
                                spacing: 6
                                Text { textFormat: Text.PlainText;
                                    Layout.fillWidth: true
                                    text: root.idl ? parent.parent.s.text : "Fetching the IDL from Logos Storage…"
                                    color: parent.parent.s.ok ? look.ink : look.dim
                                    wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(14)
                                }
                                Text { textFormat: Text.PlainText;
                                    Layout.fillWidth: true
                                    visible: parent.parent.s.instructions.length > 0
                                    text: parent.parent.s.instructions.map(Logic.clean).join("  ·  ")
                                    color: look.dim; wrapMode: Text.Wrap; font.family: look.mono; font.pixelSize: look.px(13)
                                }
                            }
                        }

                        // The reusable label, as another module would show this program.
                        RowLayout {
                            spacing: 8
                            Text { textFormat: Text.PlainText; text: "As other apps show it:"; color: look.dim; font.family: look.sans; font.pixelSize: look.px(14) }
                            RegistryLabel {
                                bridge: root.bridgeReady ? logos : null
                                programId: root.entry ? root.entry.program : ""
                                pixelSize: look.px(14)
                                fontFamily: look.sans
                                textColor: look.ink
                                quietColor: look.dim
                                verifiedColor: look.good
                                warnColor: look.alarm
                            }
                        }

                        // ── For the entry's author: prepared by the module, signed by your lezreg wallet ──
                        TechDetails {
                            id: authorTools
                            look: root.look
                            title: "Update or freeze this entry"
                            summary: "for its author"
                            color: look.slate

                            Text { textFormat: Text.PlainText;
                                Layout.fillWidth: true
                                text: "Only works when your lezreg wallet holds this entry's author key. Nothing is signed until you confirm the change below."
                                color: look.dim; wrapMode: Text.Wrap; font.family: look.sans; font.pixelSize: look.px(15)
                            }
                            // Spending on a sequencer that is not on this machine needs an explicit yes.
                            Tick {
                                look: root.look
                                visible: root.remote
                                checked: root.remoteOk
                                text: "Changes here spend real funds on " + Logic.clean(root.config ? root.config.sequencer : "")
                                onToggled: root.remoteOk = !root.remoteOk
                            }
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                Field { id: versionField; look: root.look; Layout.preferredWidth: 120; text: root.editVersion; hint: "version" }
                                Field { id: descField; look: root.look; Layout.fillWidth: true; text: root.editDescription; hint: "description" }
                            }
                            Flow {
                                Layout.fillWidth: true
                                spacing: 8
                                Action {
                                    look: root.look
                                    label: "Update entry"
                                    busyLabel: "Preparing…"
                                    changes: true; busy: root.busy; working: root.workingOn === "prepare_update"
                                    enabledNow: !!root.entry && !root.entry.frozen
                                    onPressed: root.prepare("prepare_update", { version: versionField.text.trim(), description: descField.text })
                                }
                                Action {
                                    look: root.look
                                    label: "Freeze entry"
                                    busyLabel: "Preparing…"
                                    grave: true
                                    changes: true; busy: root.busy; working: root.workingOn === "prepare_freeze"
                                    enabledNow: !!root.entry && !root.entry.frozen
                                    onPressed: root.prepare("prepare_freeze", {})
                                }
                            }

                            // The confirm sheet.
                            Rectangle {
                                visible: !!root.change
                                Layout.fillWidth: true
                                implicitHeight: sheet.implicitHeight + 32
                                radius: 8; color: look.card; border.color: look.brass
                                ColumnLayout {
                                    id: sheet
                                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: 16 }
                                    spacing: 6
                                    Repeater {
                                        model: Logic.changeLines(root.change)
                                        Text { textFormat: Text.PlainText; required property var modelData; Layout.fillWidth: true; text: modelData; color: look.ink; wrapMode: Text.WrapAnywhere; font.family: look.sans; font.pixelSize: look.px(14) }
                                    }
                                    RowLayout {
                                        spacing: 8
                                        Layout.topMargin: 8
                                        Action { look: root.look; label: "Sign and send change"; busyLabel: "Sending…"; enabledNow: root.armed; strong: true; changes: true; busy: root.busy; working: root.workingOn === "submit"
                                                 grave: !!(root.change && root.change.summary && root.change.summary.irreversible); onPressed: root.submitChange() }
                                        Action { look: root.look; label: "Discard change"; onPressed: root.change = null }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
