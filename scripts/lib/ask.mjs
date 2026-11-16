// Asking the person at the terminal: a line, a yes, or something that mustn't be shown as it is typed.
//
// Every question is Windows PowerShell's own Read-Host, in a process of its own that has the console to itself.
// Node never reads its standard input here: a question asked through Node's readline and the next one through
// Read-Host share one console input, and the second came back empty before anything was typed (the first run
// of `npm run ship -- --setup` took an empty password that way, and rightly stored nothing).

import { spawnSync } from "node:child_process";
import { isatty } from "node:tty";

/** Whether there is a terminal to ask in (not a pipe, not a workflow). Asked of the handles, without opening them. */
export const canAsk = () => isatty(0) && isatty(1);

/** Read-Host with `prompt` (it adds the colon), masked when `hidden`. Throws when it can't ask. */
function readHost(prompt, hidden) {
    const asked = `'${prompt.replace(/'/g, "''")}'`;
    const script = hidden
        ? [
              `$p = Read-Host -AsSecureString ${asked}`,
              "$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($p)",
              "try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }",
          ]
        : [`Read-Host ${asked}`];
    const result = spawnSync("powershell", ["-NoProfile", "-Command", ["[Console]::OutputEncoding = [Text.Encoding]::UTF8", ...script].join("; ")], {
        stdio: ["inherit", "pipe", "inherit"],
        encoding: "utf8",
    });
    if (result.error || result.status !== 0) throw new Error(`Couldn't ask for it (${prompt}).`);
    return result.stdout.replace(/\r?\n$/, "");
}

/** Reads a line without echoing it (stars as it is typed). */
export const askHidden = (prompt) => readHost(prompt, true);

/** Reads a line, shown as it is typed. */
export const askLine = (prompt) => readHost(prompt, false).trim();

/** A yes or a no; anything but y or yes is a no. */
export const askYes = (prompt) => /^y(es)?$/i.test(askLine(`${prompt} [y/N]`));
