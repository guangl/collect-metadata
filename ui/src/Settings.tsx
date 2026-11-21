import { useRef } from "react";
import type { KeyboardEvent } from "react";
import { Database } from "./Database";
import { EmbySettings } from "./EmbySettings";
import { LanguageSettings } from "./LanguageSettings";
import { OutputSettings } from "./OutputSettings";
import { TmdbSettings } from "./TmdbSettings";
import type { StorageStatus } from "./types";

const sections = [
  ["output", "输出与命名"],
  ["tmdb", "TMDB"],
  ["emby", "家庭 Emby"],
  ["storage", "PostgreSQL"],
  ["languages", "元数据语言"],
] as const;
export type SettingsSection = typeof sections[number][0];

export function Settings({ section, onSection, storage, onStorage, onConnected, onError }: {
  section: SettingsSection;
  onSection: (section: SettingsSection) => void;
  storage: StorageStatus;
  onStorage: (status: StorageStatus) => void;
  onConnected: () => void;
  onError: (message: string) => void;
}) {
  const buttons = useRef<Array<HTMLButtonElement | null>>([]);
  function move(event: KeyboardEvent, index: number) {
    let next = index;
    switch (event.key) {
      case "ArrowLeft": next = (index + sections.length - 1) % sections.length; break;
      case "ArrowRight": next = (index + 1) % sections.length; break;
      case "Home": next = 0; break;
      case "End": next = sections.length - 1; break;
      default: return;
    }
    event.preventDefault();
    onSection(sections[next][0]);
    buttons.current[next]?.focus();
  }

  const contents = {
    output: <OutputSettings />,
    tmdb: <TmdbSettings onError={onError} />,
    emby: <EmbySettings />,
    storage: <Database storage={storage} onStorage={onStorage} onConnected={onConnected} />,
    languages: section === "languages" ? <LanguageSettings /> : null,
  };

  return (
    <section className="preferences" aria-labelledby="settings-title">
      <div className="page-heading">
        <h1 id="settings-title">设置</h1>
        <p>管理资料来源、数据库连接和刮削结果的输出方式。</p>
      </div>
      <div className="settings-tabs" role="tablist" aria-label="设置分类">
        {sections.map(([id, label], index) => <button type="button" role="tab" key={id}
          id={"tab-" + id} aria-controls={"panel-" + id} aria-selected={section === id}
          tabIndex={section === id ? 0 : -1} ref={(button) => { buttons.current[index] = button; }}
          onKeyDown={(event) => move(event, index)} onClick={() => onSection(id)}>{label}</button>)}
      </div>
      {sections.map(([id]) => <div className="settings-panel" key={id}
        id={"panel-" + id} role="tabpanel" aria-labelledby={"tab-" + id} hidden={section !== id}>
        {contents[id]}
      </div>)}
    </section>
  );
}
