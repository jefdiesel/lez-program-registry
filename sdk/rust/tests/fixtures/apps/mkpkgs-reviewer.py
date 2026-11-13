# Builds .lgx-shaped packages (gzipped tar) for the lezreg app-check probes.
import io, json, sys, tarfile
out = sys.argv[1]
ROOT = "0a9d5c4ac2011029aabbccddeeff00112233445566778899aabbccddeeff0011"
ROOT_X = "ee" * 32
def pkg(path, manifest, files):
    with tarfile.open(path, "w:gz") as t:
        for name, data in [("manifest.json", json.dumps(manifest).encode())] + files:
            info = tarfile.TarInfo(name); info.size = len(data); t.addfile(info, io.BytesIO(data))
m = lambda author, root: {"name": "forumer_ui", "version": "1.2.0", "author": author, "type": "ui",
    "hashes": {"root": root}, "main": {"darwin-arm64": "forumer_ui_plugin.dylib"}}
evil = [("variants/darwin-arm64/forumer_ui_plugin.dylib", b"MALICIOUS CODE, NOT THE PUBLISHER'S BUILD")]
pkg(f"{out}/evil-copied-manifest.lgx", m("lns:forumer", ROOT), evil)       # genuine manifest, other files
pkg(f"{out}/brand.lgx", m("lns:brand", ROOT), evil)                          # brand forwards to forumer
pkg(f"{out}/oldfwd.lgx", m("lns:oldfwd", ROOT_X), evil)                      # oldfwd: forward "bobby.lez"
pkg(f"{out}/upper.lgx", m("lns:Forumer", ROOT), evil)                        # author not lower case
