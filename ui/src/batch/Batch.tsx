import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, FormEvent } from "react";
import { chooseBatchCandidate, chooseMediaFolder, controlBatch, createBatch, createFolderBatch, getBatchFiles, getBatchResult, getBatchView, getEmbyCatalog, importEmbyCatalog, importEmbyItem, importEmbyLibrary, isDesktop, listBatches, previewMountedFolder, retryBatchRow } from "../api";
import { languageText, PosterArt } from "../Catalog";
import { DetailDialog } from "../DetailDialog";
import { formatDate } from "../Movie";
import { errorText, mediaLabels, providerLabels } from "../types";
import type { Candidate, EmbyLibraryPreview, EmbyScrapeTarget, LookupResponse, MediaType, StorageStatus } from "../types";
import { EmbyCatalog } from "./EmbyCatalog";
import { MountedFolderPicker } from "./MountedFolderPicker";
import { AV_PREFIXES, MAX_FOLDER_FILES, MAX_ITEMS, parseBatch, terminal } from "./engine";
import type { BatchAction, BatchRow, BatchStage, BatchStatus, BatchSummary, BatchView, FolderBatchResult, FolderSelection } from "./engine";

const labels: Record<BatchStage, string> = {
  queued: "排队中",
  searching: "搜索中",
  collecting: "采集中",
  choice: "待选择",
  success: "已采集",
  partial: "部分完成",
  not_found: "未找到",
  failed: "失败",
  paused: "已暂停",
  cancelled: "已取消",
};

const batchLabels: Record<BatchStatus, string> = {
  running: "进行中",
  paused: "已暂停",
  cancelled: "已取消",
};

function CandidateChoice({ candidates, busy, onChoose }: {
  candidates: Candidate[];
  busy: boolean;
  onChoose: (candidate: Candidate) => void;
}) {
  const [selected, setSelected] = useState("");
  const candidate = selected ? candidates[Number(selected)] : undefined;
  return (
    <>
      <div className="mt-2 flex min-w-0 flex-wrap items-center gap-2">
        <label className="filter">
          候选作品
          <select className="field" value={selected} onChange={(event) => setSelected(event.target.value)}>
            <option value="">请选择作品</option>
            {candidates.map((item, index) => (
              <option value={index} key={item.provider + item.id}>
                {[item.title, item.year, languageText(item.original_languages), providerLabels[item.provider]].filter(Boolean).join(" · ")}
              </option>
            ))}
          </select>
        </label>
        <button className="secondary" type="button" disabled={!candidate || busy}
          onClick={() => { if (candidate) onChoose(candidate); }}>采集所选作品</button>
      </div>
    </>
  );
}

function LinkedFiles({ batch, row }: { batch: number; row: BatchRow }) {
  const [files, setFiles] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [limit, setLimit] = useState(100);

  async function load() {
    if (files || loading) return;
    setError(null);
    setLoading(true);
    try {
      setFiles(await getBatchFiles(batch, row.id));
    } catch (error) {
      setError(errorText(error));
    } finally {
      setLoading(false);
    }
  }

  return <details className="min-w-0 text-sm text-muted [&_ul]:my-2 [&_ul]:list-disc [&_ul]:ps-5 [&_ul]:font-mono [&_ul]:text-xs [overflow-wrap:anywhere]" onToggle={(event) => {
    if (event.currentTarget.open) void load();
  }}>
    <summary>{row.file_count} 个关联文件</summary>
    {loading && <p role="status">正在读取文件列表…</p>}
    {error && <p className="batch-error" role="alert">{error}</p>}
    {files && <ul>{files.slice(0, limit).map((file) => <li key={file}>{file}</li>)}</ul>}
    {files && files.length > limit && <button className="secondary" type="button"
      onClick={() => setLimit((value) => value + 100)}>显示更多文件</button>}
  </details>;
}

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
  const [summaries, setSummaries] = useState<BatchSummary[]>([]);
  const [listLoaded, setListLoaded] = useState(false);
  const [older, setOlder] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const [batch, setBatch] = useState<{ id: number; view: BatchView } | null>(null);
  const [action, setAction] = useState<BatchAction | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [opened, setOpened] = useState<{ id: number; response: LookupResponse; opener: HTMLButtonElement } | null>(null);
  const [changing, setChanging] = useState<number | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [folderResult, setFolderResult] = useState<FolderBatchResult | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const [selectedSource, setSelectedSource] = useState<"folder" | "emby">();
  const [embyResult, setEmbyResult] = useState<{ libraries: EmbyLibraryPreview[] | null } | { error: string }>();
  const [embyRefresh, setEmbyRefresh] = useState(0);
  const [embyImportError, setEmbyImportError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pollError, setPollError] = useState<string | null>(null);

  const refreshList = useCallback(async (before: number | null = null) => {
    const batches = await listBatches(before);
    setSummaries((previous) => before === null ? batches : [...previous, ...batches]);
    setOlder(batches.length === 50);
    setSelected((current) => current ?? batches[0]?.id ?? null);
    setListLoaded(true);
  }, []);

  useEffect(() => {
    if (!active || storage !== "ready") return;
    refreshList().catch((error) => setError(errorText(error)));
  }, [active, storage, refreshList]);

  useEffect(() => {
    if (!active || selectedSource === "folder") return;
    let stopped = false;
    setEmbyResult(undefined);
    setEmbyImportError(null);
    getEmbyCatalog().then(
      (libraries) => { if (!stopped) setEmbyResult({ libraries }); },
      (error) => { if (!stopped) setEmbyResult({ error: errorText(error) }); },
    );
    return () => { stopped = true; };
  }, [active, selectedSource, embyRefresh]);

  useEffect(() => {
    setOpened(null);
    setConfirmCancel(false);
    setPollError(null);
  }, [selected, storage]);

  useEffect(() => {
    if (!active || storage !== "ready" || selected === null) return;
    const id = selected;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      let delay = 1600;
      try {
        const view = await getBatchView(id);
        if (stopped) return;
        setBatch({ id, view });
        setSummaries((previous) => previous.some((item) => item.id === id && item.status !== view.status)
          ? previous.map((item) => item.id === id ? { ...item, status: view.status } : item)
          : previous);
        setPollError(null);
        if (view.rows.some((row) => ["queued", "searching", "collecting"].includes(row.stage))) delay = 650;
      } catch (error) {
        if (!stopped) setPollError(errorText(error));
      } finally {
        if (!stopped) timer = setTimeout(poll, delay);
      }
    }
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [active, storage, selected, refresh]);

  const view = batch?.id === selected ? batch?.view : undefined;
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
  const sourceBusy = scanning || submitting;

  useEffect(() => {
    if (!active) setOpened(null);
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
      setRefresh((value) => value + 1);
    } catch (error) {
      setError(errorText(error));
      setRefresh((value) => value + 1);
    } finally {
      setAction(null);
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (submitting || scanning || storage !== "ready") return;
    setSubmitting(true);
    setError(null);
    try {
      const id = await createBatch(parseBatch(input, kind));
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
      setFolderError("当前浏览器不支持选择文件夹。请使用支持文件夹选择的浏览器或桌面版。");
    }
  }

  function readFolder(event: ChangeEvent<HTMLInputElement>) {
    const files = event.currentTarget.files;
    if (!files?.length) return;
    if (files.length > MAX_FOLDER_FILES) {
      setFolderError("请选择包含 30,000 个以内文件的文件夹。");
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
      setRefresh((value) => value + 1);
    } catch (error) {
      setError(errorText(error));
    } finally {
      setChanging(null);
    }
  }

  const queueEmby = useCallback(async (request: () => Promise<number>, notice: string) => {
    if (sourceBusy || storage !== "ready") throw new Error("后台任务当前不可用。");
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

  async function submitEmby() {
    if (!embyCount || sourceBusy || storage !== "ready") return;
    setEmbyImportError(null);
    try {
      await queueEmby(importEmbyCatalog, "已将 Emby 的全部媒体库提交到后台任务。");
    } catch (error) {
      setEmbyImportError(errorText(error));
    }
  }

  const submitEmbyTarget = useCallback((target: EmbyScrapeTarget, name: string) => {
    const request = target.type === "library"
      ? () => importEmbyLibrary(target.id)
      : () => importEmbyItem(target.id, target.scope);
    return queueEmby(request, `已将「${name}」提交到后台任务。`);
  }, [queueEmby]);

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
        <a className="text-sm" href="#queue-title">后台任务 <span aria-hidden="true">↓</span></a>
      </div>
      {storage !== "ready" && <div className="mb-4 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-dashed border-line p-3 text-sm text-muted">
        <p>{storage === "pending" ? "正在检查 PostgreSQL…" : "连接 PostgreSQL 后即可提交后台批次。"}</p>
        {onConfigure && <button className="secondary" type="button" onClick={onConfigure}>打开设置</button>}
      </div>}
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
          {embyResult && <div className="flex flex-wrap gap-2">
            {!libraries && !embyError
              ? <button className="secondary" type="button" onClick={onConfigureEmby}>配置 Emby</button>
              : <button className="secondary" type="button" disabled={sourceBusy} onClick={() => {
                setSelectedSource("emby");
                setEmbyRefresh((value) => value + 1);
              }}>刷新媒体库</button>}
            {embyCount > 0 && <button className="primary" type="button" disabled={sourceBusy || storage !== "ready"}
              onClick={submitEmby}>{submitting ? "正在提交…" : "刮削全部媒体库"}</button>}
          </div>}
        </div>
        {!embyResult && <p className="catalog-empty" role="status">正在读取 Emby 媒体库…</p>}
        {embyError && <p className="batch-error" role="alert">{embyError}</p>}
        {embyImportError && <p className="batch-error" role="alert">{embyImportError}</p>}
        {embyResult && !libraries && !embyError && <p className="catalog-empty">连接 Emby 后会自动显示全部媒体库。</p>}
        {libraries && (libraries.length > 0
          ? <EmbyCatalog libraries={libraries} canScrape={!sourceBusy && storage === "ready"} onScrape={submitEmbyTarget} />
          : <p className="catalog-empty">Emby 尚无媒体库。</p>)}
      </section>}
      {preview && <section className="mt-5 min-w-0" aria-labelledby="folder-preview-heading">
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
        : <p className="catalog-empty">未识别到可刮削的作品。请更换文件夹。</p>}
      </section>}
      <details className="mt-4 text-sm [&>summary]:text-muted">
      <summary>手动输入作品</summary>
      <form className="panel batch-form mt-2" onSubmit={submit}>
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
      </form>
      </details>
      <section className="panel mt-6" aria-labelledby="queue-title">
      <div className="section-heading">
        <h2 id="queue-title" className="scroll-mt-5">后台任务</h2>
        {storage === "ready" && <button className="secondary" type="button" onClick={() => {
          setError(null);
          refreshList().catch((error) => setError(errorText(error)));
        }}>刷新批次</button>}
      </div>
      {error && <p className="batch-error" role="alert">{error}</p>}
      {pollError && <p className="batch-error" role="status">{pollError} 正在重试。</p>}
      {!summaries.length && !error && <p className="catalog-empty" role="status">{
        storage === "pending" || (storage === "ready" && !listLoaded) ? "正在读取后台批次…" :
          storage === "ready" ? "暂无后台任务。提交刮削后，可以在这里查看进度。" : "连接 PostgreSQL 后可查看后台任务。"
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
            <span aria-live="polite">已结束 {ended} / {rows.length}
              {running > 0 && " · 处理中 " + running}
              {waiting > 0 && " · 待选择 " + waiting}
            </span>
          </div>
          <progress value={ended} max={rows.length} aria-label="批次采集进度" />
        </div>
        {active && opened && openedRow && <DetailDialog response={opened.response}
          opener={opened.opener} closeLabel="收起结果" onDismiss={() => setOpened(null)} />}
        <ol className="poster-grid" role="list" aria-label="批次作品海报">
          {rows.map((row) => {
            const title = row.title || row.query || "缺少作品名称或番号";
            return <li className="batch-row" aria-label={row.query || "空条目"} key={row.id}>
              {row.has_result ? <button className="batch-poster" type="button"
                aria-label={"打开 " + title + " 的详情"} aria-haspopup="dialog"
                disabled={busy} onClick={(event) => open(row, event.currentTarget)}>
                <PosterArt url={row.cover_url} />
              </button> : <PosterArt url={row.cover_url} />}
              <div className="grid min-w-0 gap-2 p-2.5">
                <div className="min-w-0 [overflow-wrap:anywhere]">
                  <h3>{title}</h3>
                  <p className="muted mt-1">{row.query.startsWith("emby-episode:") ? "分集" : mediaLabels[row.kind]}{row.title && " · " + languageText(row.original_languages)}</p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="batch-status" data-stage={row.stage}>{labels[row.stage]}</span>
                  {view.status !== "cancelled" && (row.stage === "failed" || row.stage === "not_found") && row.query &&
                    <button className="secondary" type="button" disabled={busy} onClick={() => changeRow(row)}>重试</button>}
                </div>
                {row.message && <p className="muted">{row.message}</p>}
                {row.output && <div className="muted">
                  {row.output.directory && <p>已写出 {row.output.written} 项资料 · <code>{row.output.directory}</code></p>}
                  {row.output.message && <p className="batch-error">{row.output.message}</p>}
                </div>}
                {row.file_count > 0 && selected !== null && <LinkedFiles batch={selected} row={row} />}
                {row.stage === "choice" && row.candidates && <CandidateChoice candidates={row.candidates} busy={busy}
                  onChoose={(candidate) => changeRow(row, candidate)} />}
              </div>
            </li>;
          })}
        </ol>
      </>}
      </section>
    </section>
  );
}
