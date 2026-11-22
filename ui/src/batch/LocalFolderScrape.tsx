import { useState } from "react";
import { scrapeFolderWork } from "../api";
import { DetailDialog } from "../DetailDialog";
import { errorText, mediaLabels, type Candidate, type LookupResponse } from "../types";
import { CandidateChoice } from "./CandidateChoice";
import { batchStageLabels } from "./batchLabels";
import type { FolderSelection, FolderWorkResult } from "./engine";

export function LocalFolderScrape({ folder, busy, setBusy }: {
  folder: FolderSelection;
  busy: boolean;
  setBusy: (busy: boolean) => void;
}) {
  const [run, setRun] = useState<number>();
  const [results, setResults] = useState<Record<number, FolderWorkResult>>({});
  const [current, setCurrent] = useState<number>();
  const [opened, setOpened] = useState<{ response: LookupResponse; opener: HTMLButtonElement }>();
  const works = folder.preview.works;

  async function collect(batch: number, index: number, candidate: Candidate | null) {
    const { source_files } = works[index];
    setCurrent(index);
    try {
      const result = await scrapeFolderWork(batch, index + 1, source_files ?? [],
        candidate && { provider: candidate.provider, id: candidate.id });
      setResults((previous) => ({ ...previous, [index]: result }));
    } catch (error) {
      setResults((previous) => ({ ...previous, [index]: {
        stage: "failed", message: errorText(error), candidates: null, result: null, output: null,
      } }));
    } finally {
      setCurrent(undefined);
    }
  }

  async function start() {
    if (busy) return;
    setBusy(true);
    const batch = -(Date.now() * 1000 + Math.floor(Math.random() * 1000));
    setRun(batch);
    setResults({});
    try {
      for (const index of works.keys()) await collect(batch, index, null);
    } finally {
      setBusy(false);
    }
  }

  async function choose(index: number, candidate: Candidate) {
    if (busy || run === undefined) return;
    setBusy(true);
    try {
      await collect(run, index, candidate);
    } finally {
      setBusy(false);
    }
  }

  return <section className="mt-5 min-w-0" aria-labelledby="local-folder-title">
    <div className="section-heading">
      <h2 id="local-folder-title">本地文件夹刮削</h2>
      <button type="button" className="primary" disabled={busy || !works.length}
        onClick={() => void start()}>{busy ? "正在刮削…" : run ? "重新刮削" : "开始刮削元数据"}</button>
    </div>
    <p className="muted" role="status">{run
      ? `已处理 ${Object.keys(results).length} / ${works.length} 部作品`
      : `识别到 ${works.length} 部作品，关联 ${folder.preview.files} 个文件。`}</p>
    {!works.length && <p className="catalog-empty">未识别到可刮削的作品。请更换文件夹。</p>}
    <ul className="mt-3 grid gap-2" aria-label="待刮削作品">
      {works.map((work, index) => {
        const row = results[index];
        const response = row?.result;
        return <li className="panel min-w-0 space-y-2 [overflow-wrap:anywhere]" key={work.kind + ":" + work.query}>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <strong>{work.query}</strong>
            <span className="muted">{mediaLabels[work.kind]} · {work.file_count} 个文件</span>
            {current === index ? <span>正在刮削…</span> : row && <span>{batchStageLabels[row.stage]}</span>}
            {response && <button type="button" className="secondary"
              onClick={(event) => setOpened({ response, opener: event.currentTarget })}>查看结果</button>}
          </div>
          {row?.message && <p className={row.stage === "failed" ? "batch-error" : "muted"}>
            {row.stage === "failed" || row.stage === "not_found"
              ? errorText({ error: row.message, diagnostic_id: row.diagnostic_id },
                row.stage === "not_found" ? "MEDIA_NOT_FOUND" : "FOLDER_SCRAPE_FAILED")
              : row.message}
          </p>}
          {row?.candidates?.length ? <CandidateChoice candidates={row.candidates} busy={busy}
            onChoose={(candidate) => void choose(index, candidate)} /> : null}
          {row?.output?.directory && <p>已写出 {row.output.written} 项资料 · <code>{row.output.directory}</code></p>}
          {row?.output?.message && <p className="batch-error">
            {errorText({ error: row.output.message, diagnostic_id: row.diagnostic_id }, "OUTPUT_FAILED")}
          </p>}
        </li>;
      })}
    </ul>
    {opened && <DetailDialog {...opened} closeLabel="关闭本地刮削结果" onDismiss={() => setOpened(undefined)} />}
  </section>;
}
