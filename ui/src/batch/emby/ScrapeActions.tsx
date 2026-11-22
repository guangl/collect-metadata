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

export function EmbyScrapeActions({ target, name, canScrape, usesQueue, onScrape, onSubmitted }: ScrapeControls & {
  target: EmbyScrapeTarget;
  name: string;
  onSubmitted?: () => void;
}) {
  const [opener, setOpener] = useState<HTMLButtonElement>();
  const [feedback, setFeedback] = useState("");
  const scope = target.type === "library" ? "library" : target.scope;
  const children = scope === "library" ? "包含库内所有条目。" : scope === "series" ? "包含季和分集。" : "";

  async function submit(choice: EmbyScrapeChoice) {
    if (choice.mode === "overwrite") {
      await overwriteEmby(target, choice.replaceImages);
      setFeedback("Emby 已接收覆盖刮削请求。由该媒体库的元数据下载器执行。");
    } else {
      const batch = await onScrape(target, name);
      setFeedback(batch === null
        ? "Emby 已接收刮削请求。由该媒体库的元数据下载器执行。"
        : `Collect Metadata 已创建后台采集任务 #${batch}。`);
    }
    onSubmitted?.();
  }

  return <div>
    <button className="primary" type="button" aria-haspopup="dialog" disabled={!canScrape}
      onClick={(event) => { setFeedback(""); setOpener(event.currentTarget); }}>{scrapeLabels[scope]}</button>
    {feedback && <p className="muted mt-2" role="status">{feedback}</p>}
    {opener && <EmbyScrapeDialog opener={opener} target={scope === "library" ? `媒体库 · ${name}` : name} usesQueue={usesQueue}
      overwrite={`Emby 服务器调用该媒体库的元数据下载器，重新刮削并替换已有元数据。${children}`}
      onClose={() => setOpener(undefined)} onSubmit={submit} />}
  </div>;
}
