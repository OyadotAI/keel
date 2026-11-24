// Catalog skills recommended for the active project and not yet in it: counted on the sidebar's
// Extensions entry and listed first in Skills, so a recommendation is seen without opening the
// catalog. One read per project, again whenever a skill is added or removed.
import { useEffect, useState } from "react";
import { get } from "./api";
import { useStore } from "./store";

export interface CatalogSkill {
  id: string;
  description: string;
  author: string;
  present?: string | null;
  /** Why this project should have it, when it should. */
  recommended?: string | null;
}

/// Fired after a skill is added or removed, so every reader of the catalog reads it again.
export const SKILLS_CHANGED = "keel:skills-changed";

/// The recommended skills a catalog says this project lacks.
export const missing = (catalog: CatalogSkill[]) => catalog.filter((c) => c.recommended && !c.present);

/// The active project's catalog, kept current; `undefined` until read.
export function useSkillCatalog(): CatalogSkill[] | undefined {
  const ep = useStore((s) => (s.active ? s.projects[s.lanes[s.active]?.project ?? ""]?.endpoint : undefined));
  const [catalog, setCatalog] = useState<CatalogSkill[]>();
  useEffect(() => {
    if (!ep) return setCatalog(undefined);
    let live = true;
    const read = () => void get<{ entries: CatalogSkill[] }>(ep, "/api/skills/catalog").then((c) => { if (live) setCatalog(c.entries); }, () => { if (live) setCatalog([]); });
    read();
    window.addEventListener(SKILLS_CHANGED, read);
    return () => { live = false; window.removeEventListener(SKILLS_CHANGED, read); };
  }, [ep]);
  return catalog;
}
