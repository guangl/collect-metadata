import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { clearHistory, getHistory, getHistoryLanguages, getStorageStatus, loadSnapshot } from "./api";
import { Batch } from "./batch/Batch";
import type { FolderSelection } from "./batch/engine";
import { ExternalLink, LanguageSelect, languageText, PosterCard } from "./Catalog";
import { DetailDialog } from "./DetailDialog";
import { Settings } from "./Settings";
import type { SettingsSection } from "./Settings";
import { themeChoices, useTheme } from "./theme";
import type { Theme } from "./theme";
import { errorText, mediaLabels, storageLabels } from "./types";
import type { LookupResponse, MediaType, Snapshot, StorageStatus } from "./types";
import "./style.css";

const entries = [
  { name: "batch", label: "刮削工作台", icon: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" },
  { name: "history", label: "采集历史", icon: "M4 9h16v10H4zM3 5h18v4H3zM10 13h4" },
  { name: "settings", label: "设置", icon: "M4 7h5m4 0h7M4 12h10m4 0h2M4 17h2m4 0h10M9 5v4m5 1v4m-8 1v4" },
  { name: "about", label: "关于", icon: "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0ZM12 11v5m0-8h.01" },
] as const;

type View = typeof entries[number]["name"];

const shortStatus: Record<StorageStatus, string> = {
  pending: "检查中", ready: "已连接", failed: "连接异常", none: "未配置",
};

function WorkspaceHeader({ view, setView, storage, openStorage, theme, setTheme }: {
  view: View; setView: (view: View) => void; storage: StorageStatus; openStorage: () => void;
  theme: Theme; setTheme: (theme: Theme) => void;
}) {
  return (
    <header className="border-b border-line bg-header">
      <div className="mx-auto grid max-w-workspace grid-cols-[minmax(0,1fr)_auto] items-center gap-2 px-gutter py-3 lg:grid-cols-[1fr_auto_1fr] lg:gap-4">
        <button className="inline-flex min-h-11 min-w-0 items-center gap-2 justify-self-start text-start text-sm leading-tight" type="button" onClick={() => setView("batch")}>
          <img className="size-7 shrink-0 rounded-lg sm:size-[34px]" src="./icon.png" alt="" width="34" height="34" />
          <span>Collect <strong>Metadata</strong></span>
        </button>
        <nav className="col-span-full row-start-2 grid grid-cols-4 gap-1 lg:col-auto lg:row-auto" aria-label="工作区">
          {entries.map(({ name, label, icon }) => (
            <button type="button" className="nav-button px-1.5 text-[13px] sm:px-3 sm:text-sm" aria-current={view === name ? "page" : undefined}
              key={name} onClick={() => setView(name)}>
              <svg className="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65"
                strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={icon} /></svg>{label}
            </button>
          ))}
        </nav>
        <div className="flex items-center justify-self-end gap-2">
          <button type="button" className="storage" data-status={storage} title={storageLabels[storage]}
            aria-label={"打开设置，" + storageLabels[storage]} onClick={openStorage}>
            <span className="storage-dot" aria-hidden="true" />
            <span className="sm:hidden">{shortStatus[storage]}</span>
            <span className="hidden sm:inline">{storageLabels[storage]}</span>
          </button>
          <label className="theme-control">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65"
              strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d={theme === "light" ? "M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z" :
                theme === "dark" ? "M20.8 14.2A9 9 0 0 1 9.8 3.2a9 9 0 1 0 11 11Z" : "M3 5h18v12H3zM12 17v3m-4 0h8"} />
            </svg>
            <select className="field" aria-label="外观主题" value={theme} onChange={(event) => setTheme(event.target.value as Theme)}>
              {themeChoices.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </select>
          </label>
        </div>
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
      <div className="source-card max-w-2xl">
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
  onOpen: (snapshot: LookupResponse, opener: HTMLButtonElement) => void;
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
  const [languageError, setLanguageError] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);
  const [clearError, setClearError] = useState<string | null>(null);
  const [clearedCount, setClearedCount] = useState<number | null>(null);
  const [historyVersion, setHistoryVersion] = useState(0);
  const sequence = useRef(0);
  const openingSequence = useRef(0);
  const clearDialog = useRef<HTMLDialogElement>(null);
  const clearButton = useRef<HTMLButtonElement>(null);
  const cancelClearButton = useRef<HTMLButtonElement>(null);
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
  }, [canRead, historyVersion, refresh]);

  useEffect(() => {
    let active = true;
    if (canRead) getHistoryLanguages().then(
      (codes) => { if (active) { setCodes(codes); setLanguageError(null); } },
      (error) => { if (active) setLanguageError(errorText(error)); },
    );
    return () => { active = false; openingSequence.current += 1; };
  }, [canRead, historyVersion]);

  function closeClearDialog() {
    clearDialog.current?.close();
    clearButton.current?.focus();
  }

  async function confirmClear() {
    if (clearing) return;
    setClearing(true);
    setClearError(null);
    try {
      const count = await clearHistory();
      sequence.current += 1;
      setItems([]);
      setMore(false);
      setKind("");
      setLanguage("");
      setClearedCount(count);
      setHistoryVersion((version) => version + 1);
      closeClearDialog();
    } catch (error) {
      setClearError(errorText(error));
    } finally {
      setClearing(false);
    }
  }

  async function open(id: number, opener: HTMLButtonElement) {
    const request = ++openingSequence.current;
    setOpening(true);
    try {
      const snapshot = await loadSnapshot(id);
      if (request === openingSequence.current) onOpen(snapshot, opener);
    } catch (error) {
      if (request === openingSequence.current) onError(errorText(error));
    } finally {
      if (request === openingSequence.current) setOpening(false);
    }
  }

  const status = error ? "" : storage === "none" ? "连接 PostgreSQL 后即可查看采集历史。" :
    busy || storage === "pending" ? "正在读取采集历史…" : !items.length ? "没有符合条件的采集记录。" : "";

  return (
    <section aria-labelledby="history-title">
      <div className="page-heading">
        <h1 id="history-title">采集历史</h1>
        <p>按作品类型或原始语言筛选已保存的记录，打开海报查看详情。</p>
      </div>
      <div className="panel mb-4 grid gap-3 sm:flex sm:flex-wrap sm:items-center [&_.field]:flex-1">
        <label className="filter">作品类型
          <select className="field" value={kind} onChange={(event) => setKind(event.target.value as MediaType | "")} disabled={!canRead}>
            <option value="">全部类型</option>
            {(Object.keys(mediaLabels) as MediaType[]).map((type) => <option key={type} value={type}>{mediaLabels[type]}</option>)}
          </select>
        </label>
        <LanguageSelect codes={codes} value={language} onChange={setLanguage} disabled={!canRead} />
        <div className="flex flex-wrap gap-2 sm:ms-auto">
          <button type="button" className="secondary" onClick={() => refresh()} disabled={busy || !canRead}>刷新</button>
          <button type="button" className="secondary text-danger" ref={clearButton} disabled={storage !== "ready" || opening}
            onClick={() => {
              setClearError(null);
              clearDialog.current?.showModal();
              cancelClearButton.current?.focus();
            }}>清空历史</button>
        </div>
      </div>
      <dialog className="work-preview max-w-[30rem]" ref={clearDialog} aria-labelledby="clear-history-title"
        aria-describedby="clear-history-description" onCancel={(event) => {
          event.preventDefault();
          if (!clearing) closeClearDialog();
        }}>
        <div className="space-y-4 p-4 sm:p-6">
          <h2 id="clear-history-title">清空采集历史？</h2>
          <p id="clear-history-description">
            此操作将清空所有采集历史，包括当前筛选条件之外的记录，且无法撤销。后台任务仍可查看结果并写回文件。
          </p>
          {clearError && <p className="batch-error" role="alert">{clearError}</p>}
          <div className="flex flex-wrap justify-end gap-2">
            <button type="button" className="secondary" ref={cancelClearButton} disabled={clearing} onClick={closeClearDialog}>取消</button>
            <button type="button" className="secondary border-danger text-danger enabled:hover:border-danger enabled:hover:bg-danger/10"
              disabled={clearing} onClick={() => void confirmClear()}>{clearing ? "正在清空…" : "确认清空"}</button>
          </div>
        </div>
      </dialog>
      {languageError && <p className="batch-error" role="alert">{languageError}</p>}
      {error && <p className="batch-error" role="alert">{error}</p>}
      {clearedCount !== null && <p className="mb-3 text-sm text-accent" role="status">
        {clearedCount === 0 ? "采集历史已为空。" : `已清空 ${clearedCount} 条采集记录。`}
      </p>}
      {status && <p className="catalog-empty" role="status">{status}</p>}
      <div className="poster-grid">
        {items.map((item) => <PosterCard key={item.id} title={item.title || item.code}
          cover={item.cover_url} disabled={opening} onOpen={(opener) => open(item.id, opener)}
          caption={[item.year, item.scope === "episode" ? "分集" : mediaLabels[item.media_type], languageText(item.original_languages)].filter(Boolean).join(" · ")} />)}
      </div>
      {more && <button type="button" className="secondary mt-3" onClick={() => refresh(items.at(-1)?.id ?? null)}
        disabled={busy}>加载更早的记录</button>}
    </section>
  );
}

export function App() {
  const [view, setView] = useState<View>("batch");
  const [current, setCurrent] = useState<{ response: LookupResponse; opener: HTMLButtonElement } | null>(null);
  const [theme, setTheme] = useTheme();
  const [storage, setStorage] = useState<StorageStatus>("pending");
  const [notice, setNotice] = useState<{ message: string; kind?: "error" } | null>(null);
  const [databaseVersion, setDatabaseVersion] = useState(0);
  const [folderSelection, setFolderSelection] = useState<FolderSelection | null>(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("output");

  const showInfo = (message: string) => setNotice({ message });
  const showError = (message: string) => setNotice({ message, kind: "error" });

  useEffect(() => {
    getStorageStatus().then(setStorage, (error) => {
      setStorage("failed");
      setNotice({ message: errorText(error), kind: "error" });
    });
  }, []);

  function navigate(next: View) {
    if (next === view) return;
    setNotice(null);
    setView(next);
    setCurrent(null);
    window.scrollTo(0, 0);
  }

  function open(response: LookupResponse, opener: HTMLButtonElement) {
    setNotice(null);
    setCurrent({ response, opener });
    if (response.storage_error) setStorage("failed");
    else if (response.saved) setStorage("ready");
  }

  function openStorage() {
    setSettingsSection("storage");
    navigate("settings");
  }

  function openDiagnostics() {
    setSettingsSection("diagnostics");
    navigate("settings");
  }

  return (
    <div className="min-h-screen">
      <WorkspaceHeader view={view} setView={navigate} storage={storage} openStorage={openStorage} theme={theme} setTheme={setTheme} />
      <main className="min-w-0">
        {notice && <div className="mx-auto max-w-workspace px-gutter pt-3 text-sm text-accent [overflow-wrap:anywhere] data-[kind=error]:text-danger" role={notice.kind === "error" ? "alert" : "status"} data-kind={notice.kind}>
          {notice.message}
          {notice.kind === "error" && <button type="button" className="ms-2 underline" onClick={openDiagnostics}>日志与排查</button>}
        </div>}
        <div className="mx-auto max-w-workspace px-gutter pb-14 pt-6 sm:pt-9 lg:pt-12">
          <div hidden={view !== "batch"}><Batch key={databaseVersion} active={view === "batch"} storage={storage}
            folderSelection={folderSelection} setFolderSelection={setFolderSelection} onConfigure={openStorage}
            onConfigureEmby={() => { setSettingsSection("emby"); navigate("settings"); }}
            onNotice={showInfo} onError={showError} /></div>
          {view === "settings" ? (
            <Settings section={settingsSection} onSection={setSettingsSection} storage={storage}
              onStorage={setStorage} onError={showError} onConnected={() => {
                setStorage("ready");
                setDatabaseVersion((version) => version + 1);
              }} />
          ) : view === "about" ? <About onError={showError} /> : view === "history" ? <>
            <History storage={storage} onStorage={setStorage} onOpen={open} onError={showError} />
            {current && <DetailDialog response={current.response}
              opener={current.opener} closeLabel="返回采集历史" onDismiss={() => setCurrent(null)} />}
          </> : null}
        </div>
      </main>
    </div>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
