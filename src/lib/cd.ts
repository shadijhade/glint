/** What a shell is told to go to `path` (a Windows path), as typed at its prompt, Enter included. */
export function cdCommand(shell: string, path: string): string {
    // Inside single quotes: PowerShell doubles a quote, bash closes, escapes and reopens one.
    const pwsh = path.replace(/'/g, "''");
    const bash = path.replace(/'/g, `'\\''`);
    switch (shell) {
        case "pwsh":
        case "powershell":
            return `Set-Location -LiteralPath '${pwsh}'\r`;
        case "cmd":
            // A Windows path can't hold a double quote.
            return `cd /d "${path}"\r`;
        case "bash":
            return `cd "$(cygpath -u '${bash}')"\r`;
        case "wsl":
            return `cd "$(wslpath '${bash}')"\r`;
        case "nu":
            return `cd "${path.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"\r`;
        default:
            return `cd "${path}"\r`;
    }
}

/**
 * What a shell is told to run `program` (a Windows path) with `args`, as typed at its prompt, Enter
 * included: each quoted as the shell needs, so spaces and quotes in paths stay as they are.
 */
export function runLine(shell: string, program: string, args: string[]): string {
    const single = (text: string) => `'${text.replace(/'/g, "''")}'`;
    const bash = (text: string) => `'${text.replace(/'/g, `'\\''`)}'`;
    const nu = (text: string) => `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
    switch (shell) {
        case "pwsh":
        case "powershell":
            return `& ${[program, ...args].map(single).join(" ")}\r`;
        case "cmd":
            // A Windows path can't hold a double quote; neither may what's passed with it here.
            return `${[program, ...args].map((a) => `"${a.replace(/"/g, "")}"`).join(" ")}\r`;
        case "bash":
            return `"$(cygpath -u ${bash(program)})" ${args.map(bash).join(" ")}\r`;
        case "nu":
            return `^${[program, ...args].map(nu).join(" ")}\r`;
        default:
            return `"${program}" ${args.map((a) => `"${a}"`).join(" ")}\r`;
    }
}

/**
 * Whether what a terminal sends is the person typing, rather than its own reports: focus in and out
 * (ESC [I, ESC [O) and the cursor's position (ESC [row;colR).
 */
export const isTyping = (data: string) => !/^\x1b\[(?:I|O|\d+;\d+R)$/.test(data);
