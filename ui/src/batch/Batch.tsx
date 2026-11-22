import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type FormEvent } from "react";
import {
  chooseMediaFolder, createAiBatch, createBatch, createFolderBatch, getAiStatus, getEmbyCatalog,
  importEmbyCatalog, importEmbyItem, importEmbyLibrary, importEmbySelection, isDesktop, overwriteEmby,
  previewMountedFolder, refreshEmbyMetadata,
} from "../api";
import { PosterArt } from "../Catalog";
import { errorText, mediaLabels } from "../types";
import type {
  EmbyLibraryPreview, EmbyScrapeTarget, EmbyWorkPreview, MediaType, StorageStatus,
} from "../types";

import { LocalFolderScrape } from "./LocalFolderScrape";
import { MountedFolderPicker } from "./MountedFolderPicker";
import { EmbyCatalog } from "./emby/Catalog";
import { EmbyScrapeDialog, type EmbyScrapeChoice } from "./emby/ScrapeDialog";
import { AV_PREFIXES, MAX_FOLDER_FILES, MAX_ITEMS, parseBatch } from "./engine";
import type { FolderBatchResult, FolderSelection } from "./engine";

export function Batch({ active, storage, folderSelection, setFolderSelection, onConfigure, onConfigureEmby, onNotice, onBatchCreated, onOpenBatch }: {
  active: boolean;
  storage: StorageStatus;
  folderSelection: FolderSelection | null;
  setFolderSelection: (selection: FolderSelection | null) => void;
  onConfigure?: () => void;
  onConfigureEmby: () => void;
  onNotice: (message: string) => void;
  onBatchCreated: (id: number, message: string) => void;
  onOpenBatch: (id: number) => void;
}) {
  const [input, setInput] = useState("");
  const [kind, setKind] = useState<MediaType>("movie");
  const [aiConfigured, setAiConfigured] = useState(false);
  const [aiStatusError, setAiStatusError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [localBusy, setLocalBusy] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [folderResult, setFolderResult] = useState<FolderBatchResult | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const [selectedSource, setSelectedSource] = useState<"folder" | "emby">();
  const [embyResult, setEmbyResult] = useState<{ libraries: EmbyLibraryPreview[] | null } | { error: string }>();
  const [embyRefresh, setEmbyRefresh] = useState(0);
  const [allLibrariesOpener, setAllLibrariesOpener] = useState<HTMLButtonElement>();
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active) return;
    let stopped = false;
    getAiStatus().then((configured) => {
      if (!stopped) {
        setAiConfigured(configured === true);
        setAiStatusError(null);
      }
    }, (error) => {
      if (!stopped) {
        setAiConfigured(false);
        setAiStatusError("无法读取 AI 配置状态。 " + errorText(error));
      }
    });
    return () => { stopped = true; };
  }, [active]);

  useEffect(() => {
    if (!active || selectedSource === "folder") return;
    let stopped = false;
    setEmbyResult(undefined);
    getEmbyCatalog().then(
      (libraries) => { if (!stopped) setEmbyResult({ libraries }); },
      (error) => { if (!stopped) setEmbyResult({ error: errorText(error) }); },
    );
    return () => { stopped = true; };
  }, [active, selectedSource, embyRefresh]);

  const libraries = embyResult && "libraries" in embyResult ? embyResult.libraries : null;
  const embyError = embyResult && "error" in embyResult ? embyResult.error : null;
  const defaultSource = libraries || embyError ? "emby" : "folder";
  const source = selectedSource ?? (embyResult && defaultSource);
  const preview = source === "folder" ? folderSelection?.preview : undefined;
  const embyCount = useMemo(() => libraries ? new Set(
    libraries.flatMap((library) => library.works.map((work) => work.id))
  ).size : 0, [libraries]);
  const unrecognized = preview?.unrecognized ?? folderResult?.unrecognized ?? [];
  const sourceBusy = scanning || submitting || localBusy;
  const hasQueue = storage === "ready";
  const useLocalFolder = isDesktop && (storage === "none" || storage === "failed" || localBusy);

  async function submit(useAi: boolean) {
    if (submitting || scanning || storage !== "ready") return;
    setSubmitting(true);
    setError(null);
    try {
      const items = parseBatch(input, kind);
      const id = await (useAi ? createAiBatch(items) : createBatch(items));
      setInput("");
      onBatchCreated(id, `已创建${useAi ? " AI 刮削" : "后台采集"}任务 #${id}。`);
    } catch (error) {
      setError(errorText(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function scan(read: () => Promise<FolderSelection | null>) {
    if (sourceBusy) return;
    setScanning(true);
    setFolderError(null);
    try {
      const selection = await read();
      if (selection) {
        setFolderSelection(selection);
        setFolderResult(null);
      }
    } catch (error) {
      setFolderError(errorText(error));
    } finally {
      setScanning(false);
    }
  }

  async function startFolderBatch() {
    if (!folderSelection?.preview.works.length || scanning || submitting || storage !== "ready") return;
    setSubmitting(true);
    setFolderError(null);
    try {
      const result = await createFolderBatch(folderSelection.paths);
      setFolderResult(result);
      setFolderSelection(null);
      if (result.id !== null) {
        onBatchCreated(result.id, `已提交 ${result.works} 部作品，关联 ${result.files} 个文件，跳过 ${result.skipped} 个文件。`);
      }
    } catch (error) {
      setFolderError(errorText(error));
    } finally {
      setSubmitting(false);
    }
  }

  function chooseFolder() {
    if (isDesktop) {
      void scan(chooseMediaFolder);
    } else if (folderInput.current && "webkitdirectory" in folderInput.current) {
      folderInput.current.click();
    } else {
      setFolderError(errorText("当前浏览器不支持选择文件夹。请使用支持文件夹选择的浏览器或桌面版。", "FOLDER_SCAN_FAILED"));
    }
  }

  function readFolder(event: ChangeEvent<HTMLInputElement>) {
    const files = event.currentTarget.files;
    if (!files?.length) return;
    if (files.length > MAX_FOLDER_FILES) {
      setFolderError(errorText("请选择包含 30,000 个以内文件的文件夹。", "INVALID_INPUT"));
    } else {
      const paths = Array.from(files, (file) => file.webkitRelativePath || file.name);
      void scan(() => chooseMediaFolder(paths));
    }
    event.currentTarget.value = "";
  }

  const queueEmby = useCallback(async (request: () => Promise<number>, notice: string) => {
    if (sourceBusy || storage !== "ready") {
      throw new Error(errorText("后台任务当前不可用。", "BATCH_CREATE_FAILED"));
    }
    setSubmitting(true);
    try {
      const id = await request();
      onBatchCreated(id, notice);
      return id;
    } finally {
      setSubmitting(false);
    }
  }, [onBatchCreated, sourceBusy, storage]);

  const submitEmbyTarget = useCallback(async (target: EmbyScrapeTarget, name: string) => {
    if (storage !== "ready") {
      await refreshEmbyMetadata(target);
      return null;
    }
    const request = target.type === "library"
      ? () => importEmbyLibrary(target.id)
      : () => importEmbyItem(target.id, target.scope);
    return queueEmby(request, `Collect Metadata 已为「${name}」创建后台采集任务。`);
  }, [queueEmby, storage]);

  const submitEmbySelection = useCallback(async (works: EmbyWorkPreview[], choice: EmbyScrapeChoice) => {
    if (storage === "ready" && choice.mode !== "overwrite") {
      const useAi = choice.mode === "ai";
      await queueEmby(() => importEmbySelection(works.map(({ id, kind }) => ({ id, scope: kind })), useAi),
        `Collect Metadata 已为所选 ${works.length} 部作品创建${useAi ? " AI 刮削" : "后台采集"}任务。`);
      return { submitted: works.map(({ id }) => id) };
    }

    setSubmitting(true);
    const submitted: string[] = [];
    let firstFailure: string | undefined;
    try {
      for (const work of works) {
        try {
          const target = { type: "item", id: work.id, scope: work.kind } as const;
          if (choice.mode === "overwrite") await overwriteEmby(target, choice.replaceImages);
          else await refreshEmbyMetadata(target);
          submitted.push(work.id);
        } catch (error) {
          firstFailure ??= errorText(error);
        }
      }
      if (submitted.length) {
        const action = choice.mode === "overwrite" ? "覆盖刮削" : "刮削";
        onNotice(`Emby 已接收 ${submitted.length} 部作品的${action}请求。由该媒体库的元数据下载器执行。`);
      }
      return {
        submitted,
        error: firstFailure && `${works.length - submitted.length} 部作品提交失败。${firstFailure}`,
      };
    } finally {
      setSubmitting(false);
    }
  }, [queueEmby, onNotice, storage]);

  return (
    <section aria-labelledby="batch-title">
      <div className="page-heading">
        <h1 id="batch-title">刮削工作台</h1>
      </div>
      <fieldset className="media-source" aria-label="媒体来源" disabled={sourceBusy}>
        <label><input name="media-source" type="radio" checked={source === "emby"}
          onChange={() => setSelectedSource("emby")} /><span>Emby Server</span></label>
        <label><input name="media-source" type="radio" checked={source === "folder"}
          onChange={() => { setSelectedSource("folder"); setFolderError(null); }} /><span>文件夹</span></label>
      </fieldset>
      {source === undefined && <p className="catalog-empty" role="status">正在读取 Emby 媒体库…</p>}
      {source === "folder" && <section className="panel batch-form" aria-label="文件夹导入">
          {!isDesktop && <input ref={folderInput} type="file" hidden multiple
            {...{ webkitdirectory: "" }} onChange={readFolder} />}
          <div className="flex flex-wrap gap-2">
            <button className={preview ? "secondary" : "primary"} type="button" disabled={sourceBusy}
              onClick={chooseFolder}>{scanning ? "正在扫描…" : preview ? "更换媒体文件夹" : "选择媒体文件夹"}</button>
          </div>
          {isDesktop ? <p className="muted">可选择本地目录或系统已挂载的 NAS、SMB、NFS 共享。</p> : <>
            <p className="muted">浏览器只提交文件名和相对路径。也可直接浏览服务端已挂载的网络共享。</p>
            <MountedFolderPicker busy={sourceBusy} onSelect={(path) => scan(() => previewMountedFolder(path))} />
          </>}
          {folderSelection && <p className="muted">已选文件夹：<code>{folderSelection.folder}</code></p>}
        {preview && <button className="secondary" type="button" disabled={sourceBusy}
          onClick={() => { setFolderSelection(null); setFolderError(null); }}>取消预览</button>}
        {folderError && !preview && <p className="batch-error" role="alert">{folderError}</p>}
        {folderResult?.id === null && <p className="muted" role="status">
          未识别到可刮削的作品。跳过 {folderResult.skipped} 个文件。
        </p>}
        {unrecognized.length > 0 && <details className="min-w-0 text-sm text-muted [&_ul]:my-2 [&_ul]:list-disc [&_ul]:ps-5 [&_ul]:font-mono [&_ul]:text-xs [overflow-wrap:anywhere]">
          <summary>查看未识别的视频</summary>
          <ul>{unrecognized.map((path) => <li key={path}>{path}</li>)}</ul>
        </details>}
      </section>}
      {source === "emby" && <section className="min-w-0 space-y-3" aria-labelledby="emby-catalog-title" aria-busy={!embyResult}>
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line pb-4">
          <div>
            <h2 id="emby-catalog-title" className="text-xl sm:text-2xl">Emby 媒体库</h2>
            {libraries && <p className="muted mt-1" role="status">{libraries.length} 个媒体库 · {embyCount} 部作品</p>}
          </div>
          {embyResult && (!libraries && !embyError
            ? <button className="secondary" type="button" onClick={onConfigureEmby}>配置 Emby</button>
            : <div className="flex flex-wrap gap-2">
              <button className="secondary" type="button" disabled={sourceBusy} onClick={() => {
                setSelectedSource("emby");
                setEmbyRefresh((value) => value + 1);
              }}>重新读取媒体库</button>
              {hasQueue && embyCount > 0 && <button className="secondary" type="button" disabled={sourceBusy}
                aria-haspopup="dialog" onClick={(event) => setAllLibrariesOpener(event.currentTarget)}>刮削全部媒体库</button>}
            </div>)}
        </div>
        {allLibrariesOpener && <EmbyScrapeDialog opener={allLibrariesOpener} target={`全部媒体库 · ${embyCount} 部作品`}
          usesQueue onClose={() => setAllLibrariesOpener(undefined)} onSubmit={async () => {
            await queueEmby(importEmbyCatalog, "Collect Metadata 已为 Emby 的全部媒体库创建后台采集任务。");
          }} />}
        {!embyResult && <p className="catalog-empty" role="status">正在读取 Emby 媒体库…</p>}
        {embyError && <p className="batch-error" role="alert">{embyError}</p>}
        {embyResult && !libraries && !embyError && <p className="catalog-empty">连接 Emby 后会自动显示全部媒体库。</p>}
        {libraries && (libraries.length > 0
          ? <EmbyCatalog libraries={libraries} canScrape={!sourceBusy && storage !== "pending"} usesQueue={hasQueue} onScrape={submitEmbyTarget} onOpenBatch={onOpenBatch} aiConfigured={hasQueue && aiConfigured} onSelection={submitEmbySelection} />
          : <p className="catalog-empty">Emby 尚无媒体库。</p>)}
      </section>}
      {preview && folderSelection && useLocalFolder && <LocalFolderScrape key={folderSelection.folder}
        folder={folderSelection} busy={sourceBusy} setBusy={setLocalBusy} />}
      {preview && !useLocalFolder && <section className="mt-5 min-w-0" aria-labelledby="folder-preview-heading">
        <div className="section-heading items-center">
          <h2 id="folder-preview-heading">媒体海报预览</h2>
          <button className="primary" type="button" disabled={preview.works.length === 0 || sourceBusy || storage !== "ready"}
            onClick={startFolderBatch}>{submitting ? "正在提交…" : "开始刮削元数据"}</button>
        </div>
        {folderError && <p className="batch-error" role="alert">{folderError}</p>}
        <p className="muted" role="status">{`识别到 ${preview.works.length} 部作品，关联 ${preview.files} 个文件，跳过 ${preview.skipped} 个文件。`}</p>
        {preview.works.length > 0 ?
          <ul className="poster-grid" role="list" aria-label="待刮削作品">
            {preview.works.map((work) => <li className="poster-card" key={work.kind + ":" + work.query}>
              <PosterArt />
              <div className="poster-caption">
                <strong title={work.query}>{work.query}</strong>
                <span>{mediaLabels[work.kind]} · {work.file_count} 个文件</span>
              </div>
            </li>)}
          </ul>
        : <p className="catalog-empty">未识别到可刮削的作品。请检查视频扩展名、片名或季集编号，也可以手动输入作品。</p>}
      </section>}
      {error && <p className="batch-error" role="alert">{error}</p>}
      {aiStatusError && <p className="batch-error" role="alert">{aiStatusError}</p>}
      {hasQueue && <details className="mt-4 text-sm [&>summary]:text-muted">
      <summary>手动输入作品</summary>
      <form className="panel batch-form mt-2" onSubmit={(event: FormEvent<HTMLFormElement>) => { event.preventDefault(); void submit(false); }}>
        <label className="filter">默认类型
          <select className="field" value={kind} onChange={(event) => setKind(event.target.value as MediaType)}>
            {(Object.keys(mediaLabels) as MediaType[]).map((type) => (
              <option key={type} value={type}>{mediaLabels[type]}</option>
            ))}
          </select>
        </label>
        <label className="batch-input" htmlFor="batch-input">作品清单</label>
        <textarea className="field" id="batch-input" value={input} onChange={(event) => setInput(event.target.value)}
          placeholder={"电影：寄生虫\n剧集：Dark\n有码番号：ABC-123\nFC2番号：FC2-PPV-1234567"} rows={5}
          aria-describedby="batch-guide" spellCheck={false} />
        <p id="batch-guide" className="muted">每行一条，最多 {MAX_ITEMS} 条。可用「电影：」「剧集：」「番号：」前缀混合采集。AV 也可用 {AV_PREFIXES.join("、")} 作前缀，冒号后填写完整番号。</p>
        <button className="primary" type="submit" disabled={submitting || scanning || !input.trim() || storage !== "ready"}>
          {submitting ? "正在提交…" : "提交后台采集"}
        </button>
        {aiConfigured && <button className="secondary" type="button"
          disabled={submitting || scanning || !input.trim() || storage !== "ready"}
          onClick={() => void submit(true)}>提交 AI 刮削</button>}
      </form>
      </details>}
      {!hasQueue && storage !== "pending" && source === "folder" && <p className="muted mt-4">
        {isDesktop ? "后台任务和历史记录需要 PostgreSQL。" : "Web 文件夹刮削需要连接 PostgreSQL 后提交后台任务。"}
        {onConfigure && <button className="secondary ms-2" type="button" onClick={onConfigure}>数据库设置</button>}
      </p>}
    </section>
  );
}
