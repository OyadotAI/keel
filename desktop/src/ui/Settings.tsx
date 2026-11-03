// Settings separates project authority from device preferences without unmounting live controls.
import { UpdateButton } from "./UpdateButton";
import { NotificationSetting } from "./NotificationSetting";
import { BrowserPreference } from "./BrowserPreference";
import { InterfaceSetting } from "./InterfaceSetting";
import { AppearanceSetting } from "./AppearanceSetting";
import { useStore } from "../store";
import { ClaudeSection, LicenseSection, UpdatesSection } from "./SettingsServices";
import { ProjectTrust } from "./settings/ProjectTrust";
import { SettingsLayout } from "./settings/SettingsLayout";
import { useProjectConnection } from "./settings/connection";
import "./settings/Settings.css";
export { until } from "./SettingsServices";

/** Open at project trust every time while keeping unrelated preferences available offline. */
export function Settings() {
  const project = useStore((s) => (s.active ? s.lanes[s.active]?.project : undefined) ?? s.order[0]);
  const name = useStore((s) => project ? s.projects[project]?.name : undefined);
  const connection = useProjectConnection(project);
  const endpoint = connection.endpoint;
  const unavailable = !project ? "Open a project to manage this setting." : connection.error ? "The project connection is unavailable. Try again in Project trust." : "Connecting to this project…";
  return <div className="settings settings-page">
    <header className="lane-header">
      <div className="lane-heading"><span className="lane-name">Settings</span><span className="muted small">Your project, your preferences.</span></div>
      <UpdateButton />
      <button onClick={() => useStore.setState({ settings: false })}>Done</button>
    </header>
    <SettingsLayout
      trust={<ProjectTrust key={project ?? "no-project"} project={project} name={name} endpoint={endpoint} error={connection.error} retry={connection.retry} />}
      preferences={<><AppearanceSetting /><NotificationSetting /><InterfaceSetting applyToLane /><BrowserPreference /></>}
      claude={endpoint ? <ClaudeSection key={project} ep={endpoint} /> : <p className="muted">{unavailable}</p>}
      license={endpoint ? <LicenseSection key={project} ep={endpoint} /> : <section className="settings-section"><h2>License</h2><p className="muted">{unavailable}</p></section>}
      updates={<UpdatesSection />}
    />
  </div>;
}
