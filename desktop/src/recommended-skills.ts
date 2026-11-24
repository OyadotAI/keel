// What Oya Factory recommends for the active project and it does not have yet — catalog skills and
// suggested plugins — counted under Tasks in the sidebar and listed in the Recommended dialog.
// Read once per project, again whenever something is added or removed.
import { useEffect, useState } from "react";
import { get, type Endpoint } from "./api";
import { useStore } from "./store";

export interface CatalogSkill {
  id: string;
  description: string;
  author: string;
  present?: string | null;
  /** Why this project should have it, when it should. */
  recommended?: string | null;
}

export interface CatalogPlugin {
  name: string;
  description: string;
  marketplace: string;
  installed: boolean;
  reason?: string | null;
}

/// Fired after a skill or plugin is added or removed, so every reader reads again.
export const SKILLS_CHANGED = "keel:skills-changed";

/// The recommended skills a catalog says this project lacks.
export const missing = (catalog: CatalogSkill[]) => catalog.filter((c) => c.recommended && !c.present);

/// The active project's endpoint, or `undefined` before its daemon is up.
function useEndpoint(): Endpoint | undefined {
  return useStore((s) => (s.active ? s.projects[s.lanes[s.active]?.project ?? ""]?.endpoint : undefined));
}

/// `path` on the active project, read again on `SKILLS_CHANGED`; `undefined` until read.
function useRead<T>(path: string, empty: T): T | undefined {
  const ep = useEndpoint();
  const [data, setData] = useState<T>();
  useEffect(() => {
    if (!ep) return setData(undefined);
    let live = true;
    const read = () => void get<T>(ep, path).then((d) => { if (live) setData(d); }, () => { if (live) setData(empty); });
    read();
    window.addEventListener(SKILLS_CHANGED, read);
    return () => { live = false; window.removeEventListener(SKILLS_CHANGED, read); };
    // `empty` is a constant per call site.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ep, path]);
  return data;
}

const NO_SKILLS = { entries: [] as CatalogSkill[] };
const NO_PLUGINS = { suggested: [] as CatalogPlugin[], all: [] as CatalogPlugin[] };

/// The active project's skill catalog, kept current; `undefined` until read.
export function useSkillCatalog(): CatalogSkill[] | undefined {
  return useRead("/api/skills/catalog", NO_SKILLS)?.entries;
}

/// Everything recommended and missing for the active project.
export function useRecommendations(): { skills: CatalogSkill[]; plugins: CatalogPlugin[]; read: boolean } {
  const skills = useSkillCatalog();
  const plugins = useRead("/api/plugins", NO_PLUGINS)?.suggested;
  return { skills: missing(skills ?? []), plugins: (plugins ?? []).filter((p) => !p.installed), read: !!skills && !!plugins };
}
