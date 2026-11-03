// Jump navigation keeps every settings control and its in-progress work mounted in one page.
import { useState, type ReactNode } from "react";
import { Icon } from "../icons";
import { SETTINGS_DESTINATIONS } from "./constants";

interface Props { trust: ReactNode; preferences: ReactNode; claude: ReactNode; license: ReactNode; updates: ReactNode }
type Destination = typeof SETTINGS_DESTINATIONS[number]["id"];

/** Focus labelled destinations without replacing controls, losing drafts, or stopping login streams. */
export function SettingsLayout({ trust, preferences, claude, license, updates }: Props) {
  const [selected, setSelected] = useState<Destination>("trust");
  function jump(id: Destination) {
    setSelected(id);
    const heading = document.getElementById(`settings-${id}`);
    heading?.focus({ preventScroll: true });
    heading?.scrollIntoView({ block: "start" });
  }
  return <div className="settings-workspace">
    <nav className="settings-navigation" aria-label="Settings sections">
      <span className="settings-nav-caption">In this window</span>
      {SETTINGS_DESTINATIONS.map(({ id, label, icon }) => <a key={id} href={`#settings-${id}`} aria-current={selected === id ? "location" : undefined} onClick={(event) => { event.preventDefault(); jump(id); }}><Icon name={icon} size={16} />{label}</a>)}
      <p>Project trust stays with this project. Preferences apply on this device.</p>
    </nav>
    <div className="settings-body settings-content">
      <div className="settings-group settings-project-group">{trust}</div>
      <section className="settings-group" aria-labelledby="settings-preferences">
        <GroupHeading id="preferences" title="Preferences" detail="Make Keel feel like your workspace." />{preferences}
      </section>
      <section className="settings-group" aria-labelledby="settings-claude">
        <GroupHeading id="claude" title="Claude Code" detail="Installation and sign-in for your coding agent." />{claude}
      </section>
      <section className="settings-group" aria-labelledby="settings-license">
        <GroupHeading id="license" title="License & updates" detail="Your access and the version you’re running." />{license}{updates}
      </section>
    </div>
  </div>;
}

/** Stable focus targets make the jump rail usable with keyboards and assistive technology. */
function GroupHeading({ id, title, detail }: { id: Destination; title: string; detail: string }) {
  return <header className="settings-group-heading"><h2 id={`settings-${id}`} tabIndex={-1}>{title}</h2><p>{detail}</p></header>;
}
