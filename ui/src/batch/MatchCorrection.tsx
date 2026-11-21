import { useState } from "react";
import type { FormEvent } from "react";
import { searchWorks } from "../api";
import { languageText } from "../Catalog";
import { errorText, providerLabels } from "../types";
import type { Candidate, MediaProvider, MediaType } from "../types";

export function MatchCorrection({ kind, episode, busy, onSave, onClose }: {
  kind: MediaType;
  episode: boolean;
  busy: boolean;
  onSave: (candidate: { provider: MediaProvider; id: string }) => Promise<void>;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<Candidate[]>();
  const [index, setIndex] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [notes, setNotes] = useState<string[]>([]);
  const value = query.trim();
  const tmdbPattern = episode ? /^(?:tmdb\s*[:：]\s*)?([1-9][0-9]*)$/i : /^tmdb\s*[:：]\s*([1-9][0-9]*)$/i;
  const tmdbId = tmdbPattern.exec(value)?.[1];
  const candidate = tmdbId ? { provider: "tmdb" as const, id: tmdbId } :
    results?.[Number(index || -1)];
  const locked = busy || pending;

  async function search(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (locked || !value || episode || tmdbId) return;
    setPending(true);
    setError("");
    setNotes([]);
    setIndex("");
    setResults(undefined);
    try {
      const found = await searchWorks(kind, value);
      setResults(found.candidates);
      setNotes(found.notes);
    } catch (error) {
      setError(errorText(error));
    } finally {
      setPending(false);
    }
  }

  async function save() {
    if (!candidate || locked) return;
    setError("");
    try {
      await onSave({ provider: candidate.provider, id: candidate.id });
    } catch (error) {
      setError(errorText(error));
    }
  }

  return <form className="grid gap-2 rounded-lg border border-line bg-canvas p-2.5" onSubmit={search}>
    <label className="filter">{episode ? "所属剧集 TMDB ID" : "片名或 TMDB ID"}
      <input className="field" value={query} autoFocus disabled={locked}
        placeholder={episode ? "例如 70523" : "例如 Dark 或 tmdb:70523"}
        onChange={(event) => { setQuery(event.target.value); setResults(undefined); setIndex(""); setError(""); setNotes([]); }} />
    </label>
    {!episode && !tmdbId && <button className="secondary" type="submit" disabled={locked || !value}>
      {pending ? "正在搜索…" : "搜索作品"}
    </button>}
    {!!results?.length && <label className="filter">搜索结果
      <select className="field" disabled={locked} value={index} onChange={(event) => setIndex(event.target.value)}>
        <option value="">请选择作品</option>
        {results.map((item, offset) => <option value={offset} key={item.provider + item.id}>
          {[item.title, item.year, languageText(item.original_languages), providerLabels[item.provider]].filter(Boolean).join(" · ")}
        </option>)}
      </select>
    </label>}
    {results?.length === 0 && <p className="muted">没有找到作品。</p>}
    {notes.length > 0 && <p className="muted">{notes.join(" ")}</p>}
    {error && <p className="batch-error" role="alert">{error}</p>}
    <div className="flex flex-wrap gap-2">
      <button className="primary" type="button" disabled={locked || !candidate} onClick={() => void save()}>重新采集并记住</button>
      <button className="secondary" type="button" disabled={locked} onClick={onClose}>取消</button>
    </div>
  </form>;
}
