// Asking the person at the terminal: a line, a yes, or something that mustn't be shown as it is typed.

import { spawnSync } from "node:child_process";
import { createInterface } from "node:readline/promises";

/** Whether there is a terminal to ask in (not a pipe, not a workflow). */
export const canAsk = () => Boolean(process.stdin.isTTY && process.stdout.isTTY);

/** Reads a line without echoing it (Windows' own masked prompt). Throws when it can't ask. */
export function askHidden(prompt) {
    const script = [
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
        `$p = Read-Host -AsSecureString '${prompt.replace(/'/g, "''")}'`,
        "$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($p)",
        "try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }",
    ].join("; ");
    const asked = spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: ["inherit", "pipe", "inherit"], encoding: "utf8" });
    if (asked.error || asked.status !== 0) throw new Error(`Couldn't ask for it (${prompt}).`);
    return asked.stdout.replace(/\r?\n$/, "");
}

/** Reads a line, shown as it is typed. */
export async function askLine(prompt) {
    const terminal = createInterface({ input: process.stdin, output: process.stdout });
    try {
        return (await terminal.question(`${prompt}: `)).trim();
    } finally {
        terminal.close();
    }
}

/** A yes or a no; anything but y or yes is a no. */
export const askYes = async (prompt) => /^y(es)?$/i.test(await askLine(`${prompt} [y/N]`));
