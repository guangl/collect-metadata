import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type FormEvent } from "react";
import {
  chooseBatchCandidate, chooseMediaFolder, controlBatch, correctBatchMatch, createAiBatch, createBatch,
  createFolderBatch, getAiStatus, getBatchResult, getEmbyCatalog, importEmbyCatalog, importEmbyItem,
  importEmbyLibrary, importEmbySelection, isDesktop, overwriteEmby, previewMountedFolder,
  refreshEmbyMetadata, retryBatchRow,
} from "../api";
import { PosterArt } from "../Catalog";
import { DetailDialog } from "../DetailDialog";
import { formatDate } from "../Movie";
import { errorText, mediaLabels } from "../types";
import type {
  Candidate, EmbyLibraryPreview, EmbyScrapeTarget, EmbyWorkPreview, LookupResponse, MediaProvider,
  MediaType, StorageStatus,
} from "../types";

import { BatchRowCard } from "./BatchRowCard";
import { LocalFolderScrape } from "./LocalFolderScrape";
import { MountedFolderPicker } from "./MountedFolderPicker";
import { WritebackDialog } from "./WritebackDialog";
import { batchLabels } from "./batchLabels";
import { EmbyCatalog } from "./emby/Catalog";
import { EmbyScrapeDialog, type EmbyScrapeChoice } from "./emby/ScrapeDialog";
import { AV_PREFIXES, MAX_FOLDER_FILES, MAX_ITEMS, parseBatch, terminal } from "./engine";
import type { BatchAction, BatchRow, FolderBatchResult, FolderSelection } from "./engine";
import { useBatchQueue } from "./useBatchQueue";

export function Batch({ active, storage, folderSelection, setFolderSelection, onConfigure, onConfigureEmby, onNotice, onError }: {
  active: boolean;
  storage: StorageStatus;
  folderSelection: FolderSelection | null;
  setFolderSelection: (selection: FolderSelection | null) => void;
  onConfigure?: () => void;
  onConfigureEmby: () => void;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
}) {
  const [input, setInput] = useState("");
  const [kind, setKind] = useState<MediaType>("movie");
  const [action, setAction] = useState<BatchAction | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [opened, setOpened] = useState<{ id: number; response: LookupResponse; opener: HTMLButtonElement } | null>(null);
  const [aiConfigured, setAiConfigured] = useState(false);
  const [aiStatusError, setAiStatusError] = useState<string | null>(null);
  const [correcting, setCorrecting] = useState<number | null>(null);
  const [changing, setChanging] = useState<number | null>(null);
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
  const { summaries, listLoaded, older, selected, setSelected, view, pollError, refreshList, refreshBatch } =
    useBatchQueue(active, storage, setError);
  const [writeback, setWriteback] = useState<{ row: number | null; title: string } | null>(null);

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

  useEffect(() => {
    setOpened(null);
    setCorrecting(null);
    setConfirmCancel(false);
    setWriteback(null);
  }, [selected, storage]);

  const rows = view?.rows ?? [];
  const openedRow = opened && rows.find((row) => row.id === opened.id);
  const libraries = embyResult && "libraries" in embyResult ? embyResult.libraries : null;
  const embyError = embyResult && "error" in embyResult ? embyResult.error : null;
  const defaultSource = libraries || embyError ? "emby" : "folder";
  const source = selectedSource ?? (embyResult && defaultSource);
  const preview = source === "folder" ? folderSelection?.preview : undefined;
  const embyCount = useMemo(() => libraries ? new Set(
    libraries.flatMap((library) => library.works.map((work) => work.id))
  ).size : 0, [libraries]);
  const unrecognized = preview?.unrecognized ?? folderResult?.unrecognized ?? [];
  const failed = rows.filter((row) => row.stage === "failed").length;
  const ended = rows.filter(({ stage }) => terminal(stage)).length;
  const waiting = rows.filter(({ stage }) => stage === "choice").length;
  const running = rows.filter(({ stage }) => stage === "collecting" || stage === "searching").length;

  const status = view?.status;
  const unfinished = ended < rows.length;
  const canCancel = status !== "cancelled" && unfinished;
  const busy = action !== null || changing !== null;
  const sourceBusy = scanning || submitting || localBusy;
  const hasQueue = storage === "ready";
  const useLocalFolder = isDesktop && (storage === "none" || storage === "failed" || localBusy);

  useEffect(() => {
    if (!active) {
      setOpened(null);
      setWriteback(null);
    }
  }, [active]);

  async function updateBatch(next: BatchAction) {
    if (selected === null || busy) return;
    setAction(next);
    setError(null);
    try {
      const count = await controlBatch(selected, next);
      if (next === "retry_failed") {
        setOpened(null);
        onNotice("已重新排入 " + count + " 条失败记录。" + (status === "paused" ? "继续批次后开始处理。" : ""));
      }
      setConfirmCancel(false);
      refreshBatch();
    } catch (error) {
      setError(errorText(error));
      refreshBatch();
    } finally {
      setAction(null);
    }
  }

  async function submit(useAi: boolean) {
    if (submitting || scanning || storage !== "ready") return;
    setSubmitting(true);
    setError(null);
    try {
      const items = parseBatch(input, kind);
      const id = await (useAi ? createAiBatch(items) : createBatch(items));
      setSelected(id);
      setInput("");
      await refreshList();
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
    setError(null);
    try {
      const result = await createFolderBatch(folderSelection.paths);
      setFolderResult(result);
      setFolderSelection(null);
      if (result.id !== null) {
        setSelected(result.id);
        await refreshList().catch((error) => setError(errorText(error)));
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

  async function changeRow(row: BatchRow, candidate?: Candidate) {
    if (selected === null || busy) return;
    const batch = selected;
    setChanging(row.id);
    setError(null);
    try {
      if (candidate) await chooseBatchCandidate(batch, row.id, candidate);
      else await retryBatchRow(batch, row.id);
      setOpened((previous) => previous?.id === row.id ? null : previous);
      refreshBatch();
    } catch (error) {
      setError(errorText(error));
    } finally {
      setChanging(null);
    }
  }

  async function saveCorrection(row: BatchRow, candidate: { provider: MediaProvider; id: string }) {
    if (selected === null || busy) return;
    setChanging(row.id);
    try {
      await correctBatchMatch(selected, row.id, candidate);
      setOpened((previous) => previous?.id === row.id ? null : previous);
      setCorrecting(null);
      refreshBatch();
      onNotice("已提交纠正匹配。取得资料后会记住此选择。");
    } finally {
      setChanging(null);
    }
  }

  const queueEmby = useCallback(async (request: () => Promise<number>, notice: string) => {
    if (sourceBusy || storage !== "ready") {
      throw new Error(errorText("后台任务当前不可用。", "BATCH_CREATE_FAILED"));
    }
    setSubmitting(true);
    setError(null);
    try {
      const id = await request();
      setSelected(id);
      onNotice(notice);
      await refreshList().catch((error) => setError(errorText(error)));
      return id;
    } finally {
      setSubmitting(false);
    }
  }, [onNotice, refreshList, sourceBusy, storage]);

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

  async function open(row: BatchRow, opener: HTMLButtonElement) {
    if (selected === null || busy) return;
    setChanging(row.id);
    try {
      const response = await getBatchResult(selected, row.id);
      if (opener.isConnected && !opener.closest("[hidden]")) setOpened({ id: row.id, response, opener });
    } catch (error) {
      onError(errorText(error));
    } finally {
      setChanging(null);
    }
  }

  return (
    <section aria-labelledby="batch-title">
      <div className="page-heading flex flex-wrap items-baseline justify-between gap-x-4 gap-y-2">
        <h1 id="batch-title">刮削工作台</h1>
        {hasQueue && <a className="text-sm" href="#queue-title">后台任务 <span aria-hidden="true">↓</span></a>}
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
        {folderResult && <p className="muted" role="status">
          {folderResult.id === null
            ? "未识别到可刮削的作品。跳过 " + folderResult.skipped + " 个文件。"
            : "已提交 " + folderResult.works + " 部作品，关联 " + folderResult.files + " 个文件，跳过 " + folderResult.skipped + " 个文件。"}
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
          ? <EmbyCatalog libraries={libraries} canScrape={!sourceBusy && storage !== "pending"} usesQueue={hasQueue} onScrape={submitEmbyTarget} aiConfigured={hasQueue && aiConfigured} onSelection={submitEmbySelection} />
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
      {hasQueue && <section className="panel mt-6" aria-labelledby="queue-title">
      <div className="section-heading">
        <h2 id="queue-title" className="scroll-mt-5">后台任务</h2>
        <button className="secondary" type="button" onClick={() => {
          setError(null);
          refreshList().catch((error) => setError(errorText(error)));
        }}>刷新批次</button>
      </div>
      {error && <p className="batch-error" role="alert">{error}</p>}
      {pollError && <p className="batch-error" role="status">{pollError} 正在重试。</p>}
      {!summaries.length && !error && <p className="catalog-empty" role="status">{
        !listLoaded ? "正在读取后台批次…" : "暂无后台任务。提交刮削后，可以在这里查看进度。"
      }</p>}
      {summaries.length > 0 && <div className="my-3 flex flex-wrap items-center gap-x-3 gap-y-2">
        <label className="filter">已提交批次
          <select className="field" value={selected ?? ""} onChange={(event) => setSelected(Number(event.target.value))}>
            {summaries.map((batch) => (
              <option key={batch.id} value={batch.id}>#{batch.id}{batch.folder_name && " · " + batch.folder_name} · {formatDate(batch.created_at)} · {batch.total} 条{batch.status !== "running" && " · " + batchLabels[batch.status]}</option>
            ))}
          </select>
        </label>
        {older && <button className="secondary" type="button"
          onClick={() => refreshList(summaries.at(-1)?.id).catch((error) => setError(errorText(error)))}>更早批次</button>}
      </div>}
      {view && rows.length > 0 && <>
        <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2">
          <strong className="batch-state" data-status={view.status}>
            {view.status === "running" && !unfinished ? "已结束" : batchLabels[view.status]}
          </strong>
          {view.status === "running" && unfinished && <button className="secondary" type="button"
            disabled={busy} onClick={() => updateBatch("pause")}>暂停批次</button>}
          {view.status === "paused" && <button className="secondary" type="button"
            disabled={busy} onClick={() => updateBatch("resume")}>继续批次</button>}
          {canCancel && !confirmCancel && <button className="secondary batch-cancel" type="button"
            disabled={busy} onClick={() => setConfirmCancel(true)}>取消批次</button>}
          {view.status !== "cancelled" && <button className="secondary" type="button" disabled={busy || failed === 0}
            onClick={() => updateBatch("retry_failed")}>重试全部失败条目{failed > 0 && "（" + failed + "）"}</button>}
        </div>
        {canCancel && confirmCancel && <div className="mt-3 flex flex-wrap items-center gap-2 rounded-lg border border-line bg-canvas p-3 text-sm">
          <p>取消后会停止未完成的条目，并保留已采集的结果。</p>
          <button className="secondary batch-cancel" type="button" disabled={busy}
            onClick={() => updateBatch("cancel")}>确认取消</button>
          <button className="secondary" type="button" disabled={busy}
            onClick={() => setConfirmCancel(false)}>返回</button>
        </div>}
        <div className="mb-4 mt-5 [&_progress]:block [&_progress]:h-2.5 [&_progress]:w-full">
          <div className="section-heading">
            <h2>采集进度</h2>
            <span className="muted">批次 #{selected}</span>
            <span aria-live="polite">已结束 {ended} / {rows.length}
              {running > 0 && " · 处理中 " + running}
              {waiting > 0 && " · 待选择 " + waiting}
            </span>
          </div>
          <progress value={ended} max={rows.length} aria-label="批次采集进度" />
        </div>
        {status !== "cancelled" && rows.some((row) => row.writeback === "ready" || row.writeback === "failed") &&
          <button className="secondary" type="button" disabled={busy}
            onClick={() => setWriteback({ row: null, title: "此批次待写回的资料" })}>预览批次写回</button>}
        {active && writeback && selected !== null && <WritebackDialog
          key={selected + ":" + writeback.row} batch={selected} row={writeback.row} title={writeback.title}
          canQueue={status !== "cancelled"} onClose={() => setWriteback(null)}
          onQueued={(count) => {
            setWriteback(null);
            refreshBatch();
            onNotice("已排入 " + count + " 条写回任务。" + (status === "paused" ? "继续批次后开始写回。" : ""));
          }} />}
        {active && opened && openedRow && <DetailDialog response={opened.response}
          opener={opened.opener} closeLabel="收起结果" onDismiss={() => setOpened(null)} />}
        <ol className="poster-grid" role="list" aria-label="批次作品海报">
          {rows.map((row) => <BatchRowCard key={row.id}
            row={row} batch={selected} status={view.status} busy={busy} correcting={correcting === row.id}
            onOpen={(opener) => open(row, opener)} onChange={(candidate) => changeRow(row, candidate)}
            onStartCorrection={() => setCorrecting(row.id)} onCloseCorrection={() => setCorrecting(null)}
            onSaveCorrection={(candidate) => saveCorrection(row, candidate)}
            onWriteback={(title) => setWriteback({ row: row.id, title })} />)}
        </ol>
      </>}
      </section>}
    </section>
  );
}
