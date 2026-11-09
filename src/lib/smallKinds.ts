/** The files SmallPreviews.tsx shows (kept apart, so FilePreview can tell without loading that module). */
export type SmallKind = "contact" | "calendar" | "email" | "outlook" | "design" | "graphviz" | "mermaid";

const KINDS: Record<string, SmallKind> = {
    vcf: "contact",
    vcard: "contact",
    ics: "calendar",
    ical: "calendar",
    eml: "email",
    msg: "outlook",
    kra: "design",
    ora: "design",
    procreate: "design",
    sketch: "design",
    dot: "graphviz",
    gv: "graphviz",
    mmd: "mermaid",
    mermaid: "mermaid",
};

export const smallKind = (ext: string): SmallKind | undefined => KINDS[ext];
