import { useState } from "react";
import { getBatchFiles } from "../api";
import { languageText, PosterArt } from "../Catalog";
import { errorText, mediaLabels, type Candidate, type MediaProvider } from "../types";
import { CandidateChoice } from "./CandidateChoice";
import { MatchCorrection } from "./MatchCorrection";
import { batchStageLabels } from "./batchLabels";
import { terminal, type BatchRow, type BatchStatus } from "./engine";

export function BatchRowCard({
  row, batch, status, busy, correcting, onOpen, onChange, onStartCorrection, onCloseCorrection, onSaveCorrection, onWriteback,
}: {
  row: BatchRow;
  batch: number | null;
  status: BatchStatus;
  busy: boolean;
  correcting: boolean;
  onOpen: (opener: HTMLButtonElement) => void;
  onChange: (candidate?: Candidate) => Promise<void>;
  onStartCorrection: () => void;
  onCloseCorrection: () => void;
  onSaveCorrection: (candidate: { provider: MediaProvider; id: string }) => Promise<void>;
  onWriteback: (title: string) => void;
}) {
  const title = row.title || row.query || "缺少作品名称或番号";
  return <li className="batch-row" aria-label={row.query || "空条目"}>
    {row.has_result ? <button className="batch-poster" type="button"
      aria-label={"打开 " + title + " 的详情"} aria-haspopup="dialog"
      disabled={busy} onClick={(event) => onOpen(event.currentTarget)}>
      <PosterArt url={row.cover_url} />
    </button> : <PosterArt url={row.cover_url} />}
    <div className="grid min-w-0 gap-2 p-2.5">
      <div className="min-w-0 [overflow-wrap:anywhere]">
        <h3>{title}</h3>
        <p className="muted mt-1">{row.query.startsWith("emby-episode:") ? "分集" : mediaLabels[row.kind]}{row.title && " · " + languageText(row.original_languages)}</p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="batch-status" data-stage={row.stage} title={"条目 #" + row.id}>{row.writeback === "queued" ? "等待写回" : row.writeback === "collecting" ? "正在写回" : batchStageLabels[row.stage]}</span>
        {status !== "cancelled" && (row.stage === "failed" || row.stage === "not_found") && row.query &&
          <button className="secondary" type="button" disabled={busy} onClick={() => onChange()}>重试</button>}
        {status !== "cancelled" && row.kind !== "av" && row.query && !correcting &&
          row.writeback !== "queued" && row.writeback !== "collecting" &&
          (row.stage === "choice" || row.stage === "paused" || terminal(row.stage)) &&
          <button className="secondary" type="button" disabled={busy} onClick={onStartCorrection}>纠正匹配</button>}
      </div>
      {correcting && status !== "cancelled" && <MatchCorrection
        kind={row.kind} episode={row.query.startsWith("emby-episode:")} busy={busy}
        onSave={onSaveCorrection} onClose={onCloseCorrection} />}
      {(row.writeback === "ready" || row.writeback === "failed" || row.writeback === "done") &&
        <button type="button" className="secondary" disabled={busy}
          onClick={() => onWriteback(title)}>
          {row.writeback === "ready" ? "预览写回" : row.writeback === "failed" ? "查看失败并重试" : "查看写回记录"}
        </button>}
      {row.message && <p className={row.stage === "failed" ? "batch-error" : "muted"}>
        {row.stage === "failed" ? errorText(row.message, "BATCH_ITEM_FAILED")
          : row.stage === "not_found" ? errorText(row.message, "MEDIA_NOT_FOUND") : row.message}
      </p>}
      {(row.stage === "failed" || row.stage === "partial" || row.writeback === "failed") &&
        <p className="muted">条目 #{row.id}</p>}
      {row.output && <div className="muted">
        {row.output.directory && <p>已写出 {row.output.written} 项资料 · <code>{row.output.directory}</code></p>}
        {row.output.message && <p className="batch-error">
          {errorText(row.output.message, row.writeback === "failed" ? "WRITEBACK_FILE_FAILED" : "OUTPUT_FAILED")}
        </p>}
      </div>}
      {row.file_count > 0 && batch !== null && <LinkedFiles batch={batch} row={row} />}
      {row.stage === "choice" && row.candidates && <CandidateChoice candidates={row.candidates} busy={busy}
        onChoose={onChange} />}
    </div>
  </li>;
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
