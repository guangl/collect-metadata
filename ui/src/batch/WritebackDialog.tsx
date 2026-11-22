import { useEffect, useRef, useState } from "react";
import { previewWriteback, queueWriteback } from "../api";
import { errorText, type WritebackPreview } from "../types";

const kinds = { nfo: "NFO", poster: "海报", backdrop: "背景图（Fanart）", still: "分集截图" };
const actions = { create: "新增", replace: "替换并备份", blocked: "不可写入" };

export function WritebackDialog({ batch, row, title, canQueue, onClose, onQueued }: {
  batch: number;
  row: number | null;
  title: string;
  canQueue: boolean;
  onClose: () => void;
  onQueued: (count: number) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [preview, setPreview] = useState<WritebackPreview>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    const element = dialog.current;
    const opener = document.activeElement;
    element?.showModal();
    return () => {
      if (element?.open) element.close();
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    let active = true;
    previewWriteback(batch, row).then(
      (result) => { if (active) setPreview(result); },
      (error: unknown) => { if (active) setError(errorText(error)); },
    );
    return () => { active = false; };
  }, [batch, row, revision]);

  function refresh() {
    setError(undefined);
    setPreview(undefined);
    setRevision((value) => value + 1);
  }

  async function confirm() {
    if (!preview || busy) return;
    setBusy(true);
    try {
      onQueued(await queueWriteback(batch, row, preview.token));
    } catch (error) {
      setError(errorText(error));
      setPreview(undefined);
    } finally {
      setBusy(false);
    }
  }

  return <dialog className="work-preview" aria-labelledby="writeback-title" ref={dialog}
    onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="space-y-4 p-4 sm:p-6">
      <h2 id="writeback-title">Emby 媒体写回</h2>
      <p>{title}</p>
      {error && <p className="batch-error" role="alert">{error}</p>}
      {!preview && !error && <p className="muted" role="status">正在检查写回目标…</p>}
      {preview && <>
        {preview.writable > 0 && <p className="muted">旧文件会在替换前备份。每项图片只写入 Emby 返回的实际格式。</p>}
        <ul className="grid gap-2" aria-label="写回文件">
          {preview.files.map((file, index) => <li key={index} className="rounded-lg border border-line p-3 [overflow-wrap:anywhere]">
            <p><strong>{kinds[file.kind]}</strong> · <code>{file.source}</code></p>
            {file.path && <p className="muted">已写回 · <code>{file.path}</code></p>}
            {file.backup && <p className="muted">原文件备份 · <code>{file.backup}</code></p>}
            {file.error && <p className="batch-error">{errorText(file.error, "WRITEBACK_FILE_FAILED")}</p>}
            {file.destinations.length > 0 && <ul className="mt-1 grid gap-1">
              {file.destinations.map((destination) => <li key={destination.path}>
                {actions[destination.action]} · <code>{destination.path}</code>
              </li>)}
            </ul>}
          </li>)}
        </ul>
      </>}
      <div className="flex flex-wrap gap-2">
        {canQueue && preview && preview.writable > 0 &&
          <button type="button" className="primary" disabled={busy} onClick={() => void confirm()}>
            {busy ? "正在提交…" : "确认写回（" + preview.writable + " 项）"}
          </button>}
        {error && <button type="button" className="secondary" onClick={refresh}>重新预览</button>}
        <button type="button" className="secondary" disabled={busy} onClick={onClose}>关闭</button>
      </div>
    </div>
  </dialog>;
}
