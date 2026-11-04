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
  each name and an encrypted profile, so nobody can list the apps. Whoever knows the name
  finds and verifies the app. Being listed is a choice (section 5).
- **Programs come with it.** The app record names the programs the app calls, and each one
  shows its source-verification status from this registry.

## 2. The three-way binding

An app `M` (a Basecamp module name, such as `forumer_ui`) published by LNS name `N`
(such as `forumer`) counts as verified only when all three agree:

1. **The name points to the package.** `N`'s profile has the text record `app.M` (below)
   whose `root` is the package's root hash.
2. **The package points to the name.** The package's manifest has `"author": "lns:N"`. It is
   written in `metadata.json` (`author`), carried into `manifest.json`, and covered by the
   package's root hash, so it can't be changed without changing the hash.
3. **The installed package is that package.** The `hashes.root` of the installed
   `manifest.json` equals the root in `N`'s record.

Only `N`'s holder can change `N`'s profile, so a copycat can publish a package that claims
`lns:forumer` but can't make `forumer` vouch for its hash.

## 3. Records on the name (logos-names SPEC.md §7, text entries)

Key `app.<module>`: 1–32 bytes total, so module names up to 28 bytes. Value: JSON, at most
512 bytes:

```json
{"v":1,"root":"<64 hex: manifest hashes.root>","version":"0.12.0",
 "catalog":"https://raw.githubusercontent.com/<owner>/<repo>/main/logos-repo.json",
 "programs":["<base58 program id>", "..."]}
```

- `root` is required; the rest is optional. A new release changes `root` in the same
  profile update that publishes it. Several roots (`"roots":[...]`, newest first, at most 4)
  let an older install stay verified during an upgrade.
- `app.signer`: optional, the `did:key` that signs the publisher's catalog releases, for a
  catalog's `trustedSigners`. The name vouches for the key.
- One name may publish several modules (`app.names`, `app.names_ui`, ...): a profile holds
  at most 12 KB sealed.

Records are part of the name's sealed profile: readable only with the name (logos-names SPEC.md §1, §3),
never enumerable from the chain.

## 4. Checking

`verify_app(module)` on the machine running Basecamp:

1. Read the installed manifest: `<Basecamp data>/modules/<module>/manifest.json` (core) or
   `.../plugins/<module>/manifest.json` (ui). Take `author` and `hashes.root`.
2. `author` must be `lns:<N>`; otherwise the app is **unclaimed** (shown as such, not as
   unsafe).
3. Resolve `N` (one chain read), open its profile, read `app.<module>`.
4. **Verified** if the installed root is `root` or in `roots`. **Mismatch** if the record
   exists and the root differs (a copy, or an update not yet published). **Unverified** if
   `N` has no record for this module.
5. For each id in `programs`, `program_label` (this registry) adds its source status.

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
| `resolve_app` / `verify_app` in the SDK (Rust and JS, shared vectors), `lezreg app` | lez-program-registry | no |
| App listing post type on the board | logos-names (board) | the board program, on 0.4 |
| Store view in Basecamp | xAlisher's shell | — |

An on-chain app list in the registry program itself (`Entry.kind = 2`) would need a new
program, a redeploy and fresh verification evidence; it is left for a v2.

## 8. Open

- Confirm `metadata.json` `author` reaches the built manifest (the manifest has the field,
  empty today): a test build.
- The approval sheet trusts the module name the host reports for the caller; a host-provided
  package root for the caller would remove the step of reading the installed manifest.
