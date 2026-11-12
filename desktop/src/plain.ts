// Whether a lane speaks plainly. Chat lanes are for people coming from Lovable who cannot read a
// command or a branch name; terminal lanes are for engineers and keep the machine's own words.
// One place answers it, so a component never re-derives "is this a chat lane".
import { useStore, type Lane } from "./store";

/** A chat lane: plain words, no git or terminal terms. */
export const isPlain = (l?: Pick<Lane, "interface">) => !!l && l.interface !== "terminal";

/** The lane's plainness, for a component. */
export const usePlain = (lane: string) => useStore((s) => isPlain(s.lanes[lane]));

/** The folder a lane's agent works in, so a card can tell a path inside it from one outside. */
export const checkoutOf = (l: Pick<Lane, "project" | "wt">) => (l.wt ? `${l.project}/.keel/worktrees/${l.wt}` : l.project);
