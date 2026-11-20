// Signing adapter, copied from logos-sdk-js/src/signing.js (same repo family, copied not linked
// across repos). The zero-dependency JS SDK has no BIP-340 crypto of its own: bring an object
// implementing these three methods from the existing audited lez-tx.js bundle (Names app).
// Nothing is signed or sent unless a signer was explicitly configured.
// SPDX-License-Identifier: MIT OR Apache-2.0

export const SIGNING_NOT_CONFIGURED = "no signer configured: pass { signHash, accountIdOfPrivateKey, encodePublicTransaction } from the existing lez-tx.js bundle";
export const SIGNER_SOURCE = "logos-names: gateway/app-src/lez-tx.js (browser bundle at gateway/public/app/lez-tx.js)";

export function configureSigner(signer) {
  for (const method of ["signHash", "accountIdOfPrivateKey", "encodePublicTransaction"]) {
    if (typeof signer?.[method] !== "function") throw new Error(`a signer must provide ${method}()`);
  }
  current = signer;
  return current;
}
let current = null;
export const hasSigner = () => current !== null;
export function signHash(hash32, privateKey, aux = null) {
  if (!current) throw new Error(SIGNING_NOT_CONFIGURED);
  return current.signHash(hash32, privateKey, aux);
}
export function encodePublicTransaction(message, witnesses) {
  if (!current) throw new Error(SIGNING_NOT_CONFIGURED);
  return current.encodePublicTransaction(message, witnesses);
}
export function accountIdOfPrivateKey(privateKey) {
  if (!current) throw new Error(SIGNING_NOT_CONFIGURED);
  return current.accountIdOfPrivateKey(privateKey);
}
