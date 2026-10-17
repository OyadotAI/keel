/** Keep source-control cards as the reader left them across lane and app switches. */
import { create } from "zustand";

type Card = "changes" | "graph";
const key = (card: Card) => `keel.git-card.${card}`;

/** A missing or inaccessible preference leaves both cards discoverable. */
function remembered(card: Card): boolean {
  try { return localStorage.getItem(key(card)) !== "closed"; }
  catch { return true; }
}

export const useGitCards = create<{
  changes: boolean;
  graph: boolean;
  setOpen(card: Card, open: boolean): void;
}>()((set) => ({
  changes: remembered("changes"),
  graph: remembered("graph"),
  setOpen(card, open) {
    try { localStorage.setItem(key(card), open ? "open" : "closed"); }
    catch { /* Cards still toggle when preferences cannot be saved. */ }
    set({ [card]: open });
  },
}));
