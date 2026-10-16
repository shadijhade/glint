import { type ClassValue, clsx } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
    return twMerge(clsx(inputs))
}

/** Whether `value` (a version, say) is new for the notice `key`, and remembers it: shows a notice once per version. */
export function firstNotice(key: string, value: string) {
    try {
        if (localStorage.getItem(key) === value) return false;
        localStorage.setItem(key, value);
    } catch {
        // without storage a notice can repeat, which beats never showing it
    }
    return true;
}
