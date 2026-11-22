# Apps — the Program Registry browser for Basecamp

Apps is a Basecamp app that reads the on-chain Program Registry. It shows what each app on this
network does, who published it, and where its source is. You can search the list, open an app's
guide page and check whether the code still running is the code the entry describes. If your own
wallet holds an entry's author key, you can update or freeze that entry from its page.

## Install it

In Basecamp, open **Package Manager** and use **Install Local Package** twice: first the core
module `program_registry`, then this app (`registry_browser`, version 0.1.10). The app needs the
module to read the registry, so install the module first. Once installed, open **Apps** from the
sidebar.

## The index

The left column lists the apps on this network, A–Z, one name per line with a short sentence
beside it. The groups are gone: there are no section headings in the list. If several entries are
versions of one app, only the newest appears in the list; the older ones are linked on that app's
page. A program that is part of an app (such as a token program behind a wallet) is not listed as
an app in its own right; it is linked from the app it belongs to.

Use **Search apps by name** to filter the list, **Show more apps** to read the next page, and
**Refresh** to read again. If the network is busy, Apps waits and retries by itself; when it gives
up it says "Still busy" and offers **Try again**. Nothing is sent twice.

## An app's page

Pick a name to open its guide page:

- At the top: the app's title and one sentence, who published it, and buttons to **View source**, **Copy program ID** and **Copy source link**. A warning
  appears here if the program no longer runs the code the entry describes, or if nothing is
  deployed at that id. The publisher reads "by names · LNS name" when the entry's label is the
  LNS name `names` and that name's public account is the entry's author (the module checks it on
  chain); a label the chain does not confirm is in italics, "a name they gave themselves".
- **About**: the rest of the entry's description, when there is more.
- **At a glance**: the version, where the source lives, when it was listed, and who listed it.
- Links to the parts of the app, to older versions and to other versions, when there are any.
- **Technical details** (folded): the program and entry accounts, the proven author, the code's
  image, the build settings, the IDL status, whether this machine has verified the source, and how
  other apps label this program. When the IDL is unavailable, the entry still shows and the IDL
  line says why.
- **Update or freeze this entry** (also folded): for the entry's author. Apps prepares the change
  and shows exactly what it would do; nothing is signed until you press **Sign and send change**.
  A change on a sequencer that is not on this machine needs you to tick that it spends real funds
  first. Freezing cannot be undone.

Requests from other apps to change their entries appear at the top with **Approve and sign change**
and **Reject request**. The buttons rest for a moment when a card appears, so a click meant for
something else cannot land on them.

**Show network settings** shows which sequencer and registry the module reads, the active network
profile and the last failure, if any.

## How an app gets listed

Publishing a program and publishing an app under an LNS name are described in the top-level
README, under [Publish your program](../README.md#publish-your-program-one-command) and
[Apps, verified by publisher](../README.md#apps-verified-by-publisher). This app only reads those
entries; it does not create them. LEZ is the coin used for fees; Logos is the network.

## Settings

The module reads its settings from `~/.config/lez-registry/module.json`. A sequencer can be set
there:

```json
{"sequencer": "<url>"}
```
