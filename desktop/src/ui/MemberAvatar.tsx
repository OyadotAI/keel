import { Icon, type IconName } from "./icons";

const roles: Record<string, IconName> = {
  pm: "users", designer: "pencil", principal: "branch", qa: "check-circle",
  reviewer: "eye", security: "shield", reliability: "circle-dashed",
};

/** Uploaded images take precedence; built-in members share the same identity on every surface. */
export function MemberAvatar({ name, displayName, avatar, size = 20, working = false }: { name: string; displayName?: string; avatar?: string | null; size?: number; working?: boolean }) {
  return <span className={`member-avatar${working ? " is-working" : ""}`} style={{ width: size, height: size }} aria-hidden="true">
    {avatar ? <img src={avatar} alt="" /> : name === "oya-browser" ? <svg width={Math.round(size * 0.62)} height={Math.round(size * 0.62)} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><circle cx="10" cy="10" r="7" /><circle cx="14" cy="14" r="7" /></svg> : roles[name] ? <Icon name={roles[name]} size={Math.round(size * 0.62)} /> : (displayName || name).slice(0, 1).toUpperCase()}
  </span>;
}
