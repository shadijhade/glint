import { RgMatch } from "./lib/ripgrep";

export type DisplayItem =
    | { type: 'header'; file: string; matchCount: number }
    | { type: 'match'; match: RgMatch; originalIndex: number };
