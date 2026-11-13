// docs/INTEGRATE.md §3, runnable: node sdk/js/examples/label.mjs <rpc> <registry id> <program id> <storage api>
import { Registry, fetchIdl } from "../lezreg.js";   // sdk/js/lezreg.js: no dependencies

const [rpc, registry, program, storageApi] = process.argv.slice(2);
const reg = new Registry({ rpc, registry });
const label = await reg.programLabel(program);           // { text, kind, stale, ... }
const { deployer } = await reg.resolve(program);
const { status, idl } = deployer ? await fetchIdl(deployer, { storageApi }) : {};
console.log(label.text, status === "ok" ? `${idl.instructions.length} instructions` : `IDL unavailable: ${status}`);
