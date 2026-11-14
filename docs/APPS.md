# Apps: publishers you can verify, found by name

The registry already says which **program** a deployed account is and whether its source is
verified (DESIGN.md). This extends it to the **apps** people install in Basecamp, so that
LP-0023 is also how apps are found and checked, without changing the deployed registry
program. Status: design, 2026-11-04. Answers part of
[logos-co/ecosystem#270](https://github.com/logos-co/ecosystem/issues/270) (permissionless
app discovery).

## 1. What it gives

- **An app can't be spoofed.** A copy that calls itself "Forumer" fails the check, in the
  installer and on every Names Wallet approval prompt ("forumer: verified publisher").
- **A publisher can be anonymous.** The app's identity is an LNS name, which a private
  account can hold: nobody learns who publishes it, yet every update provably comes from the
  same publisher.
- **Unlisted by default, found by name.** The chain stores only an identifier derived from
  each name and a profile sealed under a key derived from it, so a name's records can't be
  read without knowing the name. That is not secrecy: every record's address is visible on
  chain (a block scan lists them), and a name that can be guessed can be found by trying it
  (Argon2 slows each guess, to about 36 a second on a laptop, but one salt serves every
  registry; a reviewer recovered 4 of 7 live names from a 1,500-word list in 141 s). **Treat an app
  record under a guessable name as public.** Being listed is a choice (section 5).
- **Programs come with it.** The app record names the programs the app calls, and each one
  shows its source-verification status from this registry.

## 2. The three-way binding

An app `M` (a Basecamp module name, such as `forumer_ui`) published by LNS name `N`
(such as `forumer`) counts as verified only when all three agree:

1. **The name points to the package.** `N`'s profile has exactly one text entry `app.M`
   (below), naming the release: its root hash, the SHA-256 of its `.lgx` and of its
   `manifest.json`, and its version.
2. **The package points to the name.** The package's manifest has `"author": "lns:N"`,
   written in `metadata.json` and carried into `manifest.json`.
3. **The package's files are that package.** The root is **recomputed from the package's
   files** (section 4) and must equal both the manifest's `hashes.root` and a root in `N`'s
   record. A package whose files don't hash to its own manifest's root is refused
   (`root_mismatch`) before any name is read.

Only `N`'s holder can change `N`'s profile, so a copycat can publish a package that claims
`lns:forumer` but can't make `forumer` vouch for its files.

**What the root covers, and what it doesn't.** The root is defined by the packaging tool,
`lgx` (logos-co/logos-package: `computeMerkleTree` in `src/crypto/signing.cpp`; "Merkle Tree
Hashing" in its `docs/spec.md`). Each file is hashed with SHA-256; a directory (`variants/<v>`,
`assets`, `docs`, `licenses`) hashes the sorted list of `path ‖ 0x00 ‖ hex(hash) ‖ \n` of its
files; `variants` hashes its variants' hashes the same way, and `root` the top-level
directories'. **`manifest.json` and `manifest.sig` are not covered.** So the root proves a
package's files, but not its manifest: the same files with another `author`, `main`,
`version` or `dependencies` have the same root. The record covers the rest: its `sha256` (of
the whole `.lgx`) proves a package file, manifest included, and its `manifest` (the SHA-256 of
`manifest.json` as lgx writes it, which is byte for byte what the package manager installs:
`lgx_get_manifest_json`) proves an installed copy's manifest. Checked: for seven lgx-built packages `lgx_get_manifest_json`
returns exactly the archive's `manifest.json` bytes, and two real Basecamp installs (chat_ui
0.3.0 from the official release, and onesecret_ui) hold exactly their package's
`manifest.json`. Without these hashes only the files are proven, and the verdict says so
(`files_verified`, never `verified`).

## 3. Records on the name (logos-names SPEC.md §7, text entries)

Key `app.<module>`: 1–32 bytes total, so module names up to 28 bytes. Value: JSON, at most
512 bytes:

```json
{"v":1,"root":"<64 hex: the package root>","sha256":"<64 hex: the .lgx file>",
 "manifest":"<64 hex: its manifest.json>","version":"0.12.0","cid":"zDvZ…",
 "keep":[{"root":"…","sha256":"…","manifest":"…","version":"0.11.0"}],
 "catalog":"https://raw.githubusercontent.com/<owner>/<repo>/main/logos-repo.json",
 "programs":["<base58 program id>", "..."]}
```

- The top-level `root`, `sha256`, `manifest` and `version` are the newest release; `keep`
  holds older releases with their own hashes, so an older install or file still verifies
  whole during an upgrade (`lezreg app record new.lgx --keep old.lgx`). At most 4 releases,
  but the 512 bytes are tight: a full release is about 250 bytes, so a record holds two with
  nothing else, or one with `catalog`, `cid` and a program (about 440 bytes). The first format's `"roots":[...]` (bare
  older roots) still reads, but a bare root can only ever give `files_verified`.
- `root` is required; `lezreg app record` always writes `sha256`, `manifest` and `version`,
  `lezreg app get` refuses a record without `sha256`, and a package whose version is not the
  release's fails (`manifest_mismatch`) whatever else matches. A key given twice, at any
  depth of the JSON, is refused.
- **`record` vouches only for a package in lgx's layout**, as far as these checks go: one gzip
  member; tar entries in lgx's order with byte-for-byte the headers lgx's
  `DeterministicTarWriter` writes; two zero blocks and nothing after; one `manifest.json` in
  lgx's canonical JSON (sorted keys, two-space indent) holding only what lgx's
  `Manifest::toJson` writes back (no unknown field such as `license`, `dependencies` and `main`
  always present, a name-only dependency as a bare string, and so on), so the manifest lgpm
  installs is byte for byte the one the record names; a SemVer `version`; and none of the
  reader's refusals (no case-insensitive name collision, no root asset overlapping a variant's
  file). It does not compare the gzip stream itself with what lgx's compressor would write.
- **Where to get it:** `cid` is the `.lgx` on Logos Storage, a download that depends on no web
  host; `catalog` is a catalog that lists it (GitHub releases, say). Either will do, because
  `sha256` (of the `.lgx` file) is checked before installing whatever the source, and the root
  is recomputed from the files of the installed copy. `lezreg app record --upload` puts the
  package on your Storage node and fills in `cid` and `sha256`; `lezreg app get <name>
  <module>` downloads it by CID and writes it only if the file's sha256 is the record's and
  its files hash to a root the record names. A Logos Storage CID (`zDvZ…`, codec `0xcd01`)
  hashes Storage's manifest of the file's block tree, not the file, so it can't be checked
  against the bytes locally: `sha256` is what binds them. A new release changes the record
  in the same profile update that publishes it.
- **Forwarding.** A name with a `forward` entry (logos-names SPEC.md §7) is checked where it
  points, one hop: a package naming `lns:nameswallet` is checked against `names` when
  `nameswallet` forwards there, and the verdict carries `forwarded_from`. Brand names can then
  lead to the publisher without copying its records. The value is normalised as a name
  (SPEC.md §2, so `names.lez` and `@Names` both mean `names`); a value that isn't a name, is
  an account ID, or is the name itself is ignored, as SPEC.md §7 says.
- **Names are normalised as SPEC.md §2 says**, in `author`, in `forward` and on the command
  line: trim, lower case, drop one `lns:`, one `@`, one `.lez`, then refuse anything that isn't
  a name or that decodes as an account ID. `lns:Forumer` names `forumer`.
- **Ambiguous records vouch for nothing.** A name whose profile has `app.M` twice, or
  `forward` twice, is refused (`ambiguous`): LNS clients disagree on which copy counts (the
  Rust ones read the first, the JavaScript ones the last). So is a `forward` or `author`
  surrounded by non-ASCII whitespace that one client trims and another doesn't (U+0085,
  U+00A0, U+FEFF).
- `app.signer`: optional, the `did:key` that signs the publisher's catalog releases, for a
  catalog's `trustedSigners`. The name vouches for the key.
- One name may publish several modules (`app.names`, `app.names_ui`, ...): a profile holds
  at most 12 KB sealed.

Records are part of the name's sealed profile: they can't be read without the name
(logos-names SPEC.md §1, §3). The record's address is visible on chain, though, and a
guessable name can be found by trying it (section 1), so treat an app record under a
guessable name as public.

## 4. Checking

`lezreg app verify <file.lgx | installed dir | its manifest.json>`, and the SDK's
`lgx::Contents` with `apps::check_following`:

1. **Recompute the root from the files.** For a `.lgx`: read it so that lgx's own reader
   can't read it differently, and refuse anything else rather than interpret it. lgx's
   `TarReader` is lenient (its `readOctal` skips leading spaces *and NULs* and stops at the
   first non-digit; it reads every header as ustar; it skips a lone zero block; `gzread` reads
   on into a second gzip member). A second review built a polyglot on that: a `manifest.sig`
   entry whose size field starts with a NUL read as size 0 by one reader and as the length of
   a whole genuine package by lgx, so one file showed the genuine package to the checker and
   another package to Basecamp. So: exactly one gzip member and nothing after it; every
   numeric header field plain octal from its first byte, then only NULs or spaces; the
   checksum lgx computes; `ustar\0` `00` magic; no bytes after a name's NUL, no link name, zero
   padding; regular files and directories only (no links, PAX or GNU headers); a directory
   with no data; two zero blocks and only zeros after them; no path twice (lgx keeps a later
   `manifest.json` or `manifest.sig`); no unsafe path; nothing the root doesn't cover but
   `manifest.json`/`manifest.sig` (a file at the top or directly under `variants/`). A
   differential test runs this reader and Basecamp's own `liblgx` over the review's packages
   (`sdk/rust/tests/fixtures/apps/differential.py`): wherever lezreg reads a package, liblgx
   extracts exactly the files lezreg hashed.
   For an installed package (`<Basecamp data>/modules/<module>` or `.../plugins/<module>`):
   hash the installed variant's files and the root `assets/` from disk (the installer's own
   `manifest.json`, `manifest.sig`, `variant` and `.lgpm-source` are listed as not covered;
   any other file counts, so an extra file changes the root), and rebuild the root with the
   manifest's hashes of the *other* variants as siblings. A bare `manifest.json` has no files:
   `verify` checks the install directory it sits in.
2. If the recomputed root isn't the manifest's `hashes.root`: **root_mismatch**, refused.
3. `author` must be `lns:<N>` (normalised); otherwise the app is **unclaimed** (shown as such,
   not as unsafe).
4. Resolve `N` (one chain read), open its profile, follow its `forward` one hop, read
   `app.<module>` (exactly one).
5. Find the releases whose root is the recomputed root (none: **mismatch**, a copy or an
   update not yet published). A release whose `version` isn't the manifest's:
   **manifest_mismatch**. Then:
   - a `.lgx`: **verified** if its SHA-256 is the release's `sha256`; **file_mismatch** if the
     release has another (the manifest or the packing was changed); without `sha256`,
     **manifest_mismatch** if the release's `manifest` isn't this `manifest.json`, else
     **files_verified**.
   - an installed copy: **verified** if its `manifest.json` is the release's `manifest`;
     **manifest_mismatch** if not; **files_verified** if the release names none.
   **ambiguous** for a key set twice; **not_published** if `N` has no record for this module.
6. For each id in `programs`, `program_label` (this registry) adds its source status.

**Exit codes and JSON.** `lezreg app verify` exits 0 for `verified` only, 3 for
`files_verified` (the files are the publisher's, the manifest is not proven: not success), and
1 for everything else. With `--json`, **consumers key on `status` only**. `publisher` and
`version` are set only for `verified`; `version` is the release's version from the record,
never the manifest's. What the package and the name
merely claim (author, the manifest's version, the record) is under `claimed`, whatever the
status. The text output never prints the manifest's version as the published one.
**JSON string values are untrusted** (they come from the package or the name): a consumer must
not print them raw. lezreg escapes control, ESC and bidi characters (as `\u{…}`) in the strings
that carry free text, `claimed.author`, `catalog` (in `claimed.record` and `app show`) and
`error`; module names, versions, hashes, CIDs and names are refused unless they are plain.
The `names set-text` line `record` prints is the one output not escaped, because it is built
only from validated parts (an LNS name, `app.<module>`, and a record whose catalog must be a
plain URL) with the value single-quoted for the shell.

The Basecamp module (`verify_app`, `verify_package`) still compares the manifest's claimed
root at commit 74a0010 and must move to these SDK calls; until then its "verified" means only
that the manifest's claim matches.

Where it shows:

- **Names Wallet approval sheet.** The provider SDK already knows the calling module's name
  (the host tells it). The sheet adds the publisher line: "forumer: verified publisher",
  "claims forumer, not confirmed", or "unclaimed".
- **Before installing.** An install view (xAlisher's Basecamp app-store prototype) checks a
  catalog's `index.json` entry the same way, comparing its `rootHash` and `manifest.author`
  with the name's record before downloading.
- **`lezreg app <name>`** prints a name's apps and their programs' status. **`lezreg app
  verify <file.lgx>`** checks a package file.

No web service is needed: step 3 is a chain read. Fetching a catalog to compare before
installing is a request to the URL the publisher chose, shown to the user.

## 5. Being listed (opt-in)

Listing is separate from identity. A publisher who wants to be found without being named
first posts an **app listing** on the LNS board: `{module, name, catalog, root, repo, cid}`,
signed by the name, with a refundable deposit. Names reply and vote. Readers rank and filter
on their own side, and nothing is deleted. A listing passes the same check as section 4, so
a copycat's listing shows as unverified. A private holder can list: the post is public and
signed by the name, but the holder stays hidden.

## 6. Threats

| Attack | Outcome |
|---|---|
| Copy of an app under the same module name, from another catalog | Its root isn't in the name's record: **mismatch** |
| Fake package claiming `lns:forumer` | Same: `forumer` doesn't vouch for its root |
| The genuine `manifest.json` copied next to other files | The root is recomputed from the files: **root_mismatch** |
| The genuine files with an edited manifest (`main`, `dependencies`, `version`, `provides`) | Same root. A `.lgx`: **file_mismatch** (`sha256`); an installed copy: **manifest_mismatch** (`manifest`), and another version than the release's is refused even against an old record. Against a record with neither hash: **files_verified**, exit 3, never `verified` |
| A polyglot `.lgx` that one tar reader reads as the genuine package and lgx as another (review B1) | Refused before any lookup: a numeric header field that isn't plain octal, a second gzip member, a lone zero block, data after the end marker; `record` vouches only for lgx's own layout |
| A name that lapses (fee mode, after its term and 30 days' grace) and is registered again by someone else | **Not detected.** The new holder can set `app.M` and vouch for anything; the check proves "the name's current holder vouches", not "the same holder as when you installed". A deposit-held name never lapses. Pinning the holder (the record's owner tag, which changes when a new holder registers the name) would catch it; not done |
| A name with two `app.M` or two `forward` entries, read differently by different clients | **ambiguous** |
| Two files whose names differ only in case (`Plugin.dylib`, `plugin.dylib`), or a root asset and a variant's own file of the same name with other bytes: one file once installed on a case-insensitive volume, holding the bytes written last (review 3) | Refused when the package is read |
| A module name, path or record field holding escape sequences, carriage returns or bidi controls, to print a fake "verified" line (review 3) | Text output escapes every untrusted string; a manifest `name` must be a module name and `version` SemVer, or the package is refused |
| A record value that breaks out of the quotes of the printed `names set-text` command (review 3) | The value is single-quoted with every `'` escaped; a non-SemVer version is refused anyway |
| A verified app depending on an unverified module | **Not covered**: dependencies are outside the proof (section 8) |
| Copycat listing on the board | Fails the check; shown unverified |
| Squatting a famous name | Names are first come, with a deposit; readers see the name's age and record. Not solved by cryptography |
| Votes from many throwaway names | Each name costs a deposit; readers weight by name age, deposit or follows. Not solved by cryptography |
| Someone edits files on your own machine | Out of scope: they already control your machine |
| Publisher loses the name's key | A new name and new records; users see the change |

## 7. Where the work goes

| Piece | Repo | Changes the deployed program? |
|---|---|---|
| `app.*` records in the spec, `names profile --app` | logos-names | no (text records exist) |
| `author: "lns:<name>"` in each module's `metadata.json` | logos-names, lez-program-registry | no |
| `verify_app` in the Names module; publisher line on the approval sheet | logos-names | no |
| `lgx::Contents`, `apps::check_following` in the Rust SDK, `lezreg app` (the JS SDK has no app checks yet) | lez-program-registry | no |
| App listing post type on the board | logos-names (board) | the board program, on 0.4 |
| Store view in Basecamp | xAlisher's shell | — |

An on-chain app list in the registry program itself (`Entry.kind = 2`) would need a new
program, a redeploy and fresh verification evidence; it is left for a v2.

## 8. Open

- ~~Confirm `metadata.json` `author` reaches the built manifest~~ **confirmed 2026-11-04**: a
  build of the names module with `"author": "lns:example"` (builder de169fd, `.#lgx-portable`)
  carries it in `manifest.json`. (That note also said "under the package's root hash"; it
  isn't: the root doesn't cover `manifest.json`. Corrected 2026-11-13.)
- ~~An installed package's manifest is not covered by anything the record holds~~ **done
  2026-11-13**: the record's `manifest` field (section 3) covers it.
- **Dependencies are outside the proof.** A verified app's `dependencies` name other modules
  by name (and version range); the check says nothing about them. A verified app can load a
  module that is unverified, or a copy installed under that name, so each module it depends on
  needs its own check (`lezreg app verify` on each, or the module's `verify_app`).
- A name that lapses and is registered again by someone else can vouch for anything
  (section 6); pinning the holder's owner tag is not done.
- The approval sheet trusts the module name the host reports for the caller; a host-provided
  package root for the caller would remove the step of reading the installed manifest.
