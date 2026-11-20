// A heading in an index ("Bridges", "Tokens", or a letter), with a hairline under it.
import QtQuick
import QtQuick.Layouts

ColumnLayout {
    id: heading
    property Look look: Look {}
    property string text: ""
    property string count: ""              // optional, quiet, after the heading
    spacing: 6
    RowLayout {
        Layout.fillWidth: true
        spacing: 8
        Text {
            textFormat: Text.PlainText
            text: heading.text.toUpperCase()
            color: look.dim
            font.family: look.sans
            font.pixelSize: look.px(13)
            font.weight: Font.Bold
            font.letterSpacing: 1.4
        }
        Text {
            textFormat: Text.PlainText
            visible: heading.count !== ""
            text: heading.count
            color: look.dim
            font.family: look.sans
            font.pixelSize: look.px(13)
        }
        Item { Layout.fillWidth: true }
    }
    Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: look.line }
}
