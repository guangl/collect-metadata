import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { chooseBatchCandidate, controlBatch, correctBatchMatch, getBatchResult, retryBatchRow } from "../api";
import { DetailDialog } from "../DetailDialog";
import { formatDate } from "../Movie";
import { errorText } from "../types";
import type { Candidate, LookupResponse, MediaProvider, StorageStatus } from "../types";
import { BatchRowCard } from "./BatchRowCard";
import { WritebackDialog } from "./WritebackDialog";
import { batchLabels } from "./batchLabels";
import { terminal, type BatchAction, type BatchRow } from "./engine";
import { useBatchQueue } from "./useBatchQueue";

export function BackgroundTasks({ active, storage, selected, setSelected, onConfigure, onNotice, onError }: {
  active: boolean;
  storage: StorageStatus;
  selected: number | null;
  setSelected: Dispatch<SetStateAction<number | null>>;
  onConfigure: () => void;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const [action, setAction] = useState<BatchAction | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [opened, setOpened] = useState<{ id: number; response: LookupResponse; opener: HTMLButtonElement } | null>(null);
  const [correcting, setCorrecting] = useState<number | null>(null);
  const [changing, setChanging] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { summaries, listLoaded, older, view, pollError, refreshList, refreshBatch } =
    useBatchQueue(active, storage, selected, setSelected, setError);
  const [writeback, setWriteback] = useState<{ row: number | null; title: string } | null>(null);

  useEffect(() => {
    setOpened(null);
    setCorrecting(null);
    setConfirmCancel(false);
    setWriteback(null);
  }, [selected, storage]);

  const rows = view?.rows ?? [];
  const openedRow = opened && rows.find((row) => row.id === opened.id);
  const failed = rows.filter((row) => row.stage === "failed").length;
  const ended = rows.filter(({ stage }) => terminal(stage)).length;
  const waiting = rows.filter(({ stage }) => stage === "choice").length;
  const running = rows.filter(({ stage }) => stage === "collecting" || stage === "searching").length;

  const status = view?.status;
  const unfinished = ended < rows.length;
  const canCancel = status !== "cancelled" && unfinished;
  const busy = action !== null || changing !== null;

  useEffect(() => {
    if (active) {
      heading.current?.focus({ preventScroll: true });
    } else {
      setOpened(null);
      setWriteback(null);
      setConfirmCancel(false);
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
    <section aria-labelledby="queue-title">
      <div className="page-heading flex flex-wrap items-center justify-between gap-3">
        <h1 id="queue-title" ref={heading} tabIndex={-1}>后台任务</h1>
        {storage === "ready" && <button className="secondary" type="button" onClick={() => {
          setError(null);
          refreshList().catch((error) => setError(errorText(error)));
        }}>刷新批次</button>}
      </div>
      {storage === "pending" ? <p className="catalog-empty" role="status">正在检查 PostgreSQL 连接…</p> :
        storage !== "ready" ? <div className="panel flex flex-wrap items-center gap-3">
          <p className="muted">{storage === "failed" ? "PostgreSQL 连接异常，无法读取后台任务。" : "连接 PostgreSQL 后即可查看后台任务。"}</p>
          <button className="secondary" type="button" onClick={onConfigure}>数据库设置</button>
        </div> : <div className="panel">
        {error && <p className="batch-error" role="alert">{error}</p>}
        {pollError && <p className="batch-error" role="status">{pollError} 正在重试。</p>}
        {!summaries.length && !error && (selected === null || !listLoaded) && <p className="catalog-empty" role="status">{
          !listLoaded ? "正在读取后台批次…" : "暂无后台任务。提交刮削后，可以在这里查看进度。"
        }</p>}
        {summaries.length > 0 && <div className="my-3 flex flex-wrap items-end gap-2">
          <label className="grid w-full max-w-3xl min-w-0 gap-2 text-sm text-muted">已提交批次
            <select className="field w-full" value={selected ?? ""} onChange={(event) => setSelected(Number(event.target.value))}>
              {selected !== null && !summaries.some((batch) => batch.id === selected) &&
                <option value={selected}>#{selected}</option>}
              {summaries.map((batch) => (
                <option key={batch.id} value={batch.id}>#{batch.id}{batch.folder_name && " · " + batch.folder_name} · {formatDate(batch.created_at)} · {batch.total} 条{batch.status !== "running" && " · " + batchLabels[batch.status]}</option>
              ))}
            </select>
          </label>
          {older && <button className="secondary" type="button"
            onClick={() => refreshList(summaries.at(-1)?.id).catch((error) => setError(errorText(error)))}>更早批次</button>}
        </div>}
        {selected !== null && !view && !pollError && listLoaded &&
          <p className="muted" role="status">正在读取批次 #{selected}…</p>}
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
      </div>}
    </section>
  );
}
