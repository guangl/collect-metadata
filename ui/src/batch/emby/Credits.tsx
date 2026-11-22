import { useEffect, useState, type ReactNode } from "react";
import { getEmbyCredits } from "../../api";
import { errorText, type EmbyCredit } from "../../types";
import { EmbyArtwork } from "./Artwork";

const creditRoles: [EmbyCredit["role"], string][] = [
  ["voice", "声优"], ["actor", "演员"], ["director", "导演"], ["writer", "编剧"], ["producer", "制片人"], ["crew", "其他幕后人员"],
];

export function EmbyCredits({ id, fallback }: { id: string; fallback?: ReactNode }) {
  const [people, setPeople] = useState<EmbyCredit[]>();
  const [error, setError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setError(undefined);
    setPeople(undefined);
    getEmbyCredits(id).then((credits) => { if (active) setPeople(credits); }, (reason) => { if (active) setError(errorText(reason)); });
    return () => { active = false; };
  }, [id, attempt]);
  if (error) return <><div className="work-panel"><p className="batch-error" role="alert">{error}</p>
    <button className="secondary" type="button" onClick={() => setAttempt((count) => count + 1)}>重新读取演职员</button></div>{fallback}</>;
  if (!people) return <p className="work-panel muted" role="status">正在读取演职员…</p>;
  if (!people.length) return fallback ?? <p className="work-panel muted">Emby 未提供该条目的演职员资料。</p>;
  return <div className="work-panel grid gap-5">
    {creditRoles.map(([role, label]) => {
      const group = people.filter((person) => person.role === role);
      return group.length > 0 && <section key={role} aria-label={label}>
        <h3 className="mb-2 text-base">{label}</h3>
        <ul className="credit-grid grid grid-cols-credits gap-2.5 sm:grid-cols-credits-wide sm:gap-3" role="list">{group.map((person, index) => <li key={`${person.id ?? person.name}-${index}`}>
          <EmbyArtwork id={person.id ?? ""} tag={person.id ? person.portrait : null} image="portrait" />
          <strong>{person.name}</strong>{person.detail && <span>{person.detail}</span>}
        </li>)}</ul>
      </section>;
    })}
  </div>;
}
