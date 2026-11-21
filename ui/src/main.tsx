import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { getHistory, getHistoryLanguages, getStorageStatus, loadSnapshot } from "./api";
import { Batch } from "./batch/Batch";
import type { FolderSelection } from "./batch/engine";
import { ExternalLink, LanguageSelect, languageText, PosterCard } from "./Catalog";
import { Movie, Sources } from "./Movie";
import { Settings } from "./Settings";
import type { SettingsSection } from "./Settings";
import { errorText, mediaLabels, storageLabels } from "./types";
import type { LookupResponse, MediaType, Snapshot, StorageStatus } from "./types";
import "./style.css";

const entries = [
  { name: "batch", label: "刮削工作台", icon: "folder-icon" },
  { name: "history", label: "采集历史", icon: "archive" },
  { name: "settings", label: "设置", icon: "settings" },
  { name: "about", label: "关于", icon: "info" },
] as const;

type View = typeof entries[number]["name"];

function WorkspaceHeader({ view, setView, storage, openStorage }: {
  view: View; setView: (view: View) => void; storage: StorageStatus; openStorage: () => void;
}) {
  return (
    <header className="workspace-header">
      <div className="workspace-header-inner">
        <button className="brand" type="button" onClick={() => setView("batch")}>
          <img src="./icon.png" alt="" width="34" height="34" />
          <span>Collect <strong>Metadata</strong></span>
        </button>
        <nav className="navigation" aria-label="工作区">
          {entries.map(({ name, label, icon }) => (
            <button type="button" className="nav-button" aria-current={view === name ? "page" : undefined}
              key={name} onClick={() => setView(name)}>
              <span className={"nav-icon " + icon} aria-hidden="true" />{label}
            </button>
          ))}
        </nav>
        <button type="button" className="storage" data-status={storage}
          aria-label={"打开设置，" + storageLabels[storage]} onClick={openStorage}>
          <span aria-hidden="true" />{storageLabels[storage]}
        </button>
      </div>
    </header>
  );
}

function About({ onError }: { onError: (message: string) => void }) {
  return (
    <section aria-labelledby="about-title">
      <div className="page-heading">
        <h1 id="about-title">关于 Collect Metadata</h1>
        <p>为媒体库批量采集、整理并导出元数据。</p>
      </div>
      <div className="source-card about-source">
        <ExternalLink value="https://www.themoviedb.org/" onError={onError}>
          <img src="./tmdb-logo.svg" width="112" height="15" alt="TMDB" />
        </ExternalLink>
        <p lang="en">This product uses the TMDB API but is not endorsed or certified by TMDB.</p>
      </div>
    </section>
  );
}

function History({ storage, onStorage, onOpen, onError }: {
  storage: StorageStatus;
  onStorage: (status: StorageStatus) => void;
  onOpen: (snapshot: LookupResponse) => void;
  onError: (message: string) => void;
}) {
  const [items, setItems] = useState<Snapshot[]>([]);
  const [more, setMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [kind, setKind] = useState<MediaType | "">("");
  const [language, setLanguage] = useState("");
  const [codes, setCodes] = useState<string[]>([]);
  const sequence = useRef(0);
  const openingSequence = useRef(0);
  const canRead = storage !== "none" && storage !== "pending";

  const refresh = useCallback(async (before: number | null = null) => {
    const request = ++sequence.current;
    setBusy(true);
    setError(null);
    if (before === null) {
      setItems([]);
      setMore(false);
    }
    try {
      const page = await getHistory(before, kind || null, language || null);
      if (request !== sequence.current) return;
      setItems((previous) => before === null ? page : [...previous, ...page]);
      setMore(page.length === 50);
      onStorage("ready");
    } catch (error) {
      if (request !== sequence.current) return;
      setError(errorText(error));
      onStorage("failed");
    } finally {
      if (request === sequence.current) setBusy(false);
    }
  }, [onStorage, kind, language]);

  useEffect(() => {
    if (canRead) refresh();
    return () => { sequence.current += 1; };
  }, [canRead, refresh]);

  useEffect(() => {
    if (canRead) getHistoryLanguages().then(setCodes, () => {});
    return () => { openingSequence.current += 1; };
  }, [canRead]);

  async function open(id: number) {
    const request = ++openingSequence.current;
    setOpening(true);
    try {
      const snapshot = await loadSnapshot(id);
      if (request === openingSequence.current) onOpen(snapshot);
    } catch (error) {
      if (request === openingSequence.current) onError(errorText(error));
    } finally {
      if (request === openingSequence.current) setOpening(false);
    }
  }

  const status = storage === "none" ? "连接 PostgreSQL 后即可查看采集历史。" :
    busy || storage === "pending" ? "正在读取采集历史…" : error || (!items.length ? "没有符合条件的采集记录。" : "");

  return (
    <section className="history-section" aria-labelledby="history-title">
      <div className="page-heading">
        <h1 id="history-title">采集历史</h1>
        <p>按作品类型或原始语言筛选已保存的记录，打开海报查看详情。</p>
      </div>
      <div className="filters history-filters">
        <label className="filter">作品类型
          <select value={kind} onChange={(event) => setKind(event.target.value as MediaType | "")} disabled={!canRead}>
            <option value="">全部类型</option>
            {(Object.keys(mediaLabels) as MediaType[]).map((type) => <option key={type} value={type}>{mediaLabels[type]}</option>)}
          </select>
        </label>
        <LanguageSelect codes={codes} value={language} onChange={setLanguage} disabled={!canRead} />
        <button type="button" className="secondary" onClick={() => refresh()} disabled={busy || !canRead}>刷新</button>
      </div>
      {status && <p className="catalog-empty" role="status">{status}</p>}
      <div className="poster-grid">
        {items.map((item) => <PosterCard key={item.id} title={item.title || item.code}
          cover={item.cover_url} disabled={opening} onOpen={() => open(item.id)}
          caption={[item.year, mediaLabels[item.media_type], languageText(item.original_languages)].filter(Boolean).join(" · ")} />)}
      </div>
      {more && <button type="button" className="secondary history-more" onClick={() => refresh(items.at(-1)?.id ?? null)}
        disabled={busy}>加载更早的记录</button>}
    </section>
  );
}

export function App() {
  const [view, setView] = useState<View>("batch");
  const [current, setCurrent] = useState<LookupResponse | null>(null);
  const [storage, setStorage] = useState<StorageStatus>("pending");
  const [notice, setNotice] = useState<{ message: string; kind?: "error" } | null>(null);
  const [resultVersion, setResultVersion] = useState(0);
  const [databaseVersion, setDatabaseVersion] = useState(0);
  const [folderSelection, setFolderSelection] = useState<FolderSelection | null>(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("output");

  const showInfo = (message: string) => setNotice({ message });
  const showError = (message: string) => setNotice({ message, kind: "error" });

  useEffect(() => {
    getStorageStatus().then(setStorage, () => setStorage("failed"));
  }, []);

  function navigate(next: View) {
    if (next === view) return;
    setNotice(null);
    setView(next);
    setCurrent(null);
    window.scrollTo(0, 0);
  }

  function open(response: LookupResponse) {
    setNotice(null);
    setCurrent(response);
    setResultVersion((version) => version + 1);
    if (response.saved) setStorage("ready");
    if (response.storage_error) {
      setStorage("failed");
      showError(response.storage_error + " 可以导出本次结果。");
    }
    window.scrollTo(0, 0);
  }

  function openStorage() {
    setSettingsSection("storage");
    navigate("settings");
  }

  return (
    <div className="workspace">
      <WorkspaceHeader view={view} setView={navigate} storage={storage} openStorage={openStorage} />
      <main className="main">
        {notice && <div className="notice" role="status" data-kind={notice.kind}>{notice.message}</div>}
        <div className="content">
          <div hidden={view !== "batch"}><Batch key={databaseVersion} active={view === "batch"} storage={storage}
            folderSelection={folderSelection} setFolderSelection={setFolderSelection} onConfigure={openStorage} onNotice={showInfo} onError={showError} /></div>
          {view === "settings" ? (
            <Settings section={settingsSection} onSection={setSettingsSection} storage={storage}
              onStorage={setStorage} onError={showError} onConnected={() => {
                setStorage("ready");
                setDatabaseVersion((version) => version + 1);
              }} />
          ) : view === "about" ? <About onError={showError} /> : view === "history" ? <>
            <div hidden={Boolean(current)}><History storage={storage} onStorage={setStorage} onOpen={open} onError={showError} /></div>
            {current && <>
              <button className="secondary back-results" type="button" onClick={() => setCurrent(null)}>返回采集历史</button>
              <Movie key={resultVersion} response={current} onNotice={showInfo} onError={showError} />
              <Sources key={"history-" + resultVersion} kind={current.record.media_type ?? "av"}
                sources={current.record.sources} loading={false} onError={showError} />
            </>}
          </> : null}
        </div>
      </main>
    </div>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
