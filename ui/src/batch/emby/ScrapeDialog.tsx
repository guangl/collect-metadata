import { useId, useLayoutEffect, useRef, useState } from "react";
import { errorText } from "../../types";

export type EmbyScrapeChoice =
  | { mode: "standard" | "ai" }
  | { mode: "overwrite"; replaceImages: boolean };

export function EmbyScrapeDialog({ opener, fallbackFocus, target, usesQueue, overwrite, allowAi, onClose, onSubmit }: {
  opener: HTMLButtonElement;
  fallbackFocus?: HTMLButtonElement | null;
  target: string;
  usesQueue: boolean;
  overwrite?: string;
  allowAi?: boolean;
  onClose: () => void;
  onSubmit: (choice: EmbyScrapeChoice) => Promise<void>;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const field = useId();
  const [mode, setMode] = useState<EmbyScrapeChoice["mode"]>("standard");
  const [replaceImages, setReplaceImages] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const modes = [
    {
      value: "standard",
      label: usesQueue ? "Collect Metadata 后台采集" : "Emby 普通刮削",
      hint: usesQueue
        ? "由 Collect Metadata 采集资料，结果可在后台任务中查看。"
        : "由 Emby 服务器按媒体库配置调用元数据下载器。",
    },
    ...(allowAi ? [{
      value: "ai",
      label: "Collect Metadata AI 刮削",
      hint: "由 Collect Metadata 采集，匹配不明确时用 AI 辅助查找作品。",
    }] as const : []),
    ...(overwrite ? [{
      value: "overwrite",
      label: "Emby 覆盖刮削",
      hint: overwrite,
    }] as const : []),
  ] as const;
  const submitLabel = mode === "overwrite" ? "确认覆盖刮削" : usesQueue ? "提交采集任务" : "请求 Emby 刮削";

  useLayoutEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => {
      if (element?.open) element.close();
      const destination = opener.isConnected && !opener.disabled ? opener : fallbackFocus;
      destination?.focus({ preventScroll: true });
    };
  }, [opener, fallbackFocus]);

  async function submit() {
    if (pending) return;
    setPending(true);
    setError("");
    try {
      await onSubmit(mode === "overwrite" ? { mode, replaceImages } : { mode });
      onClose();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setPending(false);
    }
  }

  return <dialog className="work-preview scrape-dialog" ref={dialog} aria-labelledby={title}
    onCancel={(event) => { event.preventDefault(); event.stopPropagation(); if (!pending) onClose(); }}>
    <form className="grid gap-4 p-4 sm:p-6" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <div className="[overflow-wrap:anywhere]">
        <h2 id={title}>{modes.length > 1 ? "选择刮削方式" : "确认刮削"}</h2>
        <p className="mt-1 text-sm text-muted">{target}</p>
      </div>
      {modes.length > 1 ? <fieldset className="grid gap-2" disabled={pending}>
        <legend className="sr-only">刮削方式</legend>
        {modes.map(({ value, label, hint }, index) => <label key={value}
          className="flex cursor-pointer items-start gap-3 rounded-lg border border-line p-3"
          data-selected={mode === value}>
          <input className="mt-1 size-4 shrink-0" type="radio" name={field} value={value}
            aria-labelledby={`${field}-label-${index}`} aria-describedby={`${field}-${index}`} checked={mode === value} onChange={() => {
              setMode(value);
              setReplaceImages(false);
              setError("");
            }} />
          <span>
            <strong id={`${field}-label-${index}`} className="block text-sm">{label}</strong>
            <span id={`${field}-${index}`} className="mt-1 block text-sm text-muted">{hint}</span>
          </span>
        </label>)}
      </fieldset> : <p className="text-sm text-muted">{modes[0].hint}</p>}
      {mode === "overwrite" && <label className="check-option">
        <input type="checkbox" checked={replaceImages} disabled={pending}
          onChange={(event) => setReplaceImages(event.target.checked)} />
        同时替换已有图片
      </label>}
      {error && <p role="alert" className="batch-error">{error}</p>}
      <div className="flex flex-wrap justify-end gap-2">
        <button className="secondary" type="button" disabled={pending} onClick={onClose}>取消</button>
        <button className={mode === "overwrite" ? "secondary batch-cancel" : "primary"} type="submit" disabled={pending}>
          {pending ? "正在提交…" : submitLabel}
        </button>
      </div>
    </form>
  </dialog>;
}
