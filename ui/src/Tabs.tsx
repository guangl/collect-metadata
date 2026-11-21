import { useId, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

export function Tabs({ label, tabs, className = "" }: {
  label: string;
  tabs: { id: string; label: string; content: ReactNode; retain?: boolean; panelFocusable?: boolean }[];
  className?: string;
}) {
  const prefix = useId();
  const [selected, setSelected] = useState<string>();
  const [visited, setVisited] = useState<string[]>(() => tabs.length ? [tabs[0].id] : []);
  const active = tabs.some(({ id }) => id === selected) ? selected : tabs[0]?.id;
  function select(id: string) {
    setSelected(id);
    setVisited((old) => old.includes(id) ? old : [...old, id]);
  }
  function move(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const last = tabs.length - 1;
    const next = event.key === "ArrowLeft" ? (index + last) % tabs.length
      : event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "Home" ? 0 : event.key === "End" ? last : -1;
    if (next < 0) return;
    event.preventDefault();
    const target = event.currentTarget.parentElement?.children[next] as HTMLButtonElement | undefined;
    select(tabs[next].id);
    target?.focus();
  }
  if (!tabs.length) return null;
  return <div className={`tab-view min-w-0 ${className}`}>
    <div className="tab-strip" role="tablist" aria-label={label}>
      {tabs.map((tab, index) => <button className="tab-button" type="button" key={tab.id} id={`${prefix}-tab-${index}`}
        role="tab" aria-selected={tab.id === active} aria-controls={`${prefix}-panel-${index}`}
        tabIndex={tab.id === active ? 0 : -1} onClick={() => select(tab.id)} onKeyDown={(event) => move(event, index)}>{tab.label}</button>)}
    </div>
    {tabs.map((tab, index) => <div key={tab.id} role="tabpanel" id={`${prefix}-panel-${index}`}
      aria-labelledby={`${prefix}-tab-${index}`} hidden={tab.id !== active}
      tabIndex={tab.id === active && tab.panelFocusable !== false ? 0 : undefined}>
      {(tab.id === active || (tab.retain && visited.includes(tab.id))) && tab.content}
    </div>)}
  </div>;
}
