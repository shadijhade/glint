// Says whether a release can be made with nobody there (docs/releasing.md, "A release that needs nobody"):
// whether the update key and its password sign an update the app accepts, and whether the Microsoft Store takes
// the Partner Center application's sign-in. It takes them from the environment, by the names every release
// script uses, so on GitHub (the Release check workflow) it tests the repository's secrets themselves, which
// nothing else can: GitHub tells nobody whether a secret is empty or what it holds.
//
// It signs a scrap in a temporary folder, signs in to the Store and reads the app. It changes nothing, and
// prints neither the key, its password nor the application's key.
//
// Usage: node scripts/release-check.mjs
//   TAURI_SIGNING_PRIVATE_KEY (the key's text, or its path), TAURI_SIGNING_PRIVATE_KEY_PASSWORD
//   PARTNER_CENTER_TENANT_ID, PARTNER_CENTER_CLIENT_ID, PARTNER_CENTER_CLIENT_SECRET
// Exit code 0 when the key works and the Store's application works or isn't set; 1 otherwise.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { KEY_SAYS, STORE_SAYS, keyState, markLine, sound, storeState } from "./lib/hands-off.mjs";
import { root, updatePubkey } from "./lib/project.mjs";

const env = process.env;
const store = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "store.json"), "utf8"));

const key = await keyState({
    key: env.TAURI_SIGNING_PRIVATE_KEY || env.TAURI_SIGNING_PRIVATE_KEY_PATH,
    password: env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
    publicKey: updatePubkey(),
});
const app = await storeState({
    tenantId: env.PARTNER_CENTER_TENANT_ID,
    clientId: env.PARTNER_CENTER_CLIENT_ID,
    clientSecret: env.PARTNER_CENTER_CLIENT_SECRET,
    appId: store.storeId,
});

// "ok" is good news, "unset" a part that was never set up (a release goes on without it), anything else is broken.
const tell = (state, title, says, detail) => {
    const [sign, level] = state === "ok" ? ["✓", "notice"] : state === "unset" ? ["!", "warning"] : ["✗", "error"];
    const line = `${says}${detail ? `: ${detail}` : ""}`;
    console.log(`${sign} ${title}: ${line}`);
    // On GitHub the same sentence is on the run's page, where a person looks first.
    if (env.GITHUB_ACTIONS) console.log(`::${level} title=${title}::${line}`);
};
// The Store's own words are worth reading (which app, what is missing, what Microsoft answered); the key's state says it all.
tell(key.state, "The update key", KEY_SAYS[key.state]);
tell(app.state, "The Microsoft Store", STORE_SAYS[app.state], app.state === "unset" ? undefined : app.detail);
console.log(markLine({ key, store: app }));

if (!sound({ key, store: app })) {
    console.log("\nnpm run ship -- --setup puts right what is missing (docs/releasing.md).");
    process.exit(1);
}
