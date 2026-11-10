# Signing test vectors

These files are for `scripts/lib/minisign-compare.test.mjs` (`npm run test:signing`). **The keys here are throwaway test keys: nothing trusts them, they never signed a release, and they must never be used for one.** The password of `test-pw.key` is `w14-test-password` (it is in the test, too); `test-empty.key` has none.

They were made once, with the Tauri CLI Glint used to ship with (`tauri signer generate --ci -p <password> -w <key>`), and the small `stand-in.exe` was signed with each (`TAURI_SIGNING_PRIVATE_KEY_PATH=<key> tauri signer sign --app-version 7.8.9 stand-in.exe`):

| File | What it is |
|------|------------|
| `test-pw.key`, `test-pw.key.pub` | A minisign key pair, secret key encrypted with a password |
| `test-empty.key`, `test-empty.key.pub` | The same, with an empty password |
| `stand-in.exe` | A small file standing in for an installer (no trailing newline, so line-ending conversion can't touch it) |
| `stand-in.pw.tauri.sig`, `stand-in.empty.tauri.sig` | What the Tauri CLI wrote for it: the format `minisign.mjs` has to match and the app has to accept |
| `stand-in.pw.ours.sig`, `stand-in.empty.ours.sig` | What `minisign.mjs` wrote for it under the same trusted comment (kept so a change in our signing shows up as a byte difference) |

Tauri's signer mixes random noise into the Ed25519 nonce, so its `.sig` bytes can't be reproduced; the test compares layout, key id and algorithm, and checks that both signatures verify with `checkSignature`, the check `verify-update.mjs` and the app run. The CLI is no longer needed to run the test.

To make new vectors, install `@tauri-apps/cli` in a scratch folder (not in this repository), repeat the commands above, and regenerate the `ours` files by running the test with `UPDATE_VECTORS=1`.
