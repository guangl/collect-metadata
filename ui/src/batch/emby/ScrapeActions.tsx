import { useState } from "react";
import { overwriteEmby } from "../../api";
import type { EmbyScrapeTarget } from "../../types";
import { EmbyScrapeDialog, type EmbyScrapeChoice } from "./ScrapeDialog";

export type ScrapeControls = {
  canScrape: boolean;
  usesQueue: boolean;
  onScrape: (target: EmbyScrapeTarget, name: string) => Promise<number | null>;
};

const scrapeLabels = {
  library: "刮削此媒体库",
  movie: "刮削此影片",
  series: "刮削此剧集",
  episode: "刮削此分集",
};

export function EmbyScrapeActions({ target, name, canScrape, usesQueue, onScrape, onSubmitted, onOpenBatch }: ScrapeControls & {
  target: EmbyScrapeTarget;
  name: string;
  onSubmitted?: () => void;
  onOpenBatch?: (id: number) => void;
}) {
  const [opener, setOpener] = useState<HTMLButtonElement>();
  const [feedback, setFeedback] = useState<{ message: string; batch?: number }>();
  const batchId = feedback?.batch;
  const scope = target.type === "library" ? "library" : target.scope;
  const children = scope === "library" ? "包含库内所有条目。" : scope === "series" ? "包含季和分集。" : "";

  async function submit(choice: EmbyScrapeChoice) {
    if (choice.mode === "overwrite") {
      await overwriteEmby(target, choice.replaceImages);
      setFeedback({ message: "Emby 已接收覆盖刮削请求。由该媒体库的元数据下载器执行。" });
    } else {
      const batch = await onScrape(target, name);
      setFeedback(batch === null
        ? { message: "Emby 已接收刮削请求。由该媒体库的元数据下载器执行。" }
        : { message: `Collect Metadata 已创建后台采集任务 #${batch}。`, batch });
    }
    onSubmitted?.();
  }

  return <div>
    <button className="primary" type="button" aria-haspopup="dialog" disabled={!canScrape}
      onClick={(event) => { setFeedback(undefined); setOpener(event.currentTarget); }}>{scrapeLabels[scope]}</button>
    {feedback && <div className="mt-2 flex flex-wrap items-baseline gap-x-2 gap-y-1">
      <p className="muted" role="status">{feedback.message}</p>
      {batchId !== undefined && onOpenBatch && <button type="button" className="text-sm text-accent underline"
        onClick={() => onOpenBatch(batchId)}>查看批次 #{batchId}</button>}
    </div>}
    {opener && <EmbyScrapeDialog opener={opener} target={scope === "library" ? `媒体库 · ${name}` : name} usesQueue={usesQueue}
      overwrite={`Emby 服务器调用该媒体库的元数据下载器，重新刮削并替换已有元数据。${children}`}
      onClose={() => setOpener(undefined)} onSubmit={submit} />}
  </div>;
}
