import { useState } from "react";
import type { ReactNode, MouseEvent } from "react";
import { isDesktop, openUrl } from "./api";
import type { Episode } from "./types";

const languageNames = new Intl.DisplayNames(["zh-Hans"], { type: "language" });
const specificNames: Record<string, string> = {
  cmn: "普通话",
  yue: "粤语",
  zh: "中文（未细分）",
  "zh-CN": "简体中文",
  "zh-TW": "繁體中文（台灣）",
  "zh-HK": "繁體中文（香港）",
  zxx: "无语言内容",
  und: "未知",
};

export function languageName(code: string): string {
  try {
    return specificNames[code] ?? languageNames.of(code) ?? code;
  } catch {
    return code;
  }
}

export function languageText(codes?: readonly string[]): string {
  return codes?.length ? codes.map(languageName).join(" · ") : languageName("und");
}

export function LanguageSelect({ codes, value, onChange, disabled }: {
  codes: readonly string[];
  value: string;
  onChange: (language: string) => void;
  disabled?: boolean;
}) {
  const options = [...new Set(codes)].filter((code) => code !== "und")
    .sort((a, b) => languageName(a).localeCompare(languageName(b), "zh-Hans"));
  return (
    <label className="filter">
      原始语言
      <select className="field" value={value} onChange={(event) => onChange(event.target.value)} disabled={disabled}>
        <option value="">全部语言</option>
        {options.map((code) => <option key={code} value={code}>{languageName(code)}</option>)}
        <option value="und">未知</option>
      </select>
    </label>
  );
}

export function safeUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (url.protocol === "https:" && !url.username && !url.password) return url.href;
  } catch {
    return null;
  }
  return null;
}

export function ExternalLink({ value, children, className, onError }: {
  value: string;
  children: ReactNode;
  className?: string;
  onError: (message: string) => void;
}) {
  const url = safeUrl(value);
  if (!url) return <span>{value}</span>;

  async function open(event: MouseEvent<HTMLAnchorElement>) {
    event.preventDefault();
    if (!url) return;
    try {
      await openUrl(url);
    } catch {
      onError("无法打开来源链接。");
    }
  }

  return <a href={url} target="_blank" rel="noopener noreferrer" className={className}
    onClick={isDesktop ? open : undefined}>{children}</a>;
}

export function PosterArt({ url }: { url?: string | null }) {
  const poster = safeUrl(url);
  const [failed, setFailed] = useState<string | null>(null);
  return (
    <span className="poster-art" aria-hidden="true">
      <span className="poster-placeholder">无海报</span>
      {poster && failed !== poster && <img src={poster} alt="" loading="lazy" decoding="async"
        referrerPolicy="no-referrer" onError={() => setFailed(poster)} />}
    </span>
  );
}

export function PosterCard({ title, cover, caption, disabled, onOpen }: {
  title: string;
  cover?: string | null;
  caption: string;
  disabled: boolean;
  onOpen: (opener: HTMLButtonElement) => void;
}) {
  return (
    <button className="poster-card" type="button" aria-haspopup="dialog" disabled={disabled}
      onClick={(event) => onOpen(event.currentTarget)}>
      <PosterArt url={cover} />
      <span className="poster-caption">
        <strong>{title}</strong>
        <span>{caption}</span>
      </span>
    </button>
  );
}

export function LocalDate({ value }: { value: string }) {
  let label = value;
  if (/^\d{4}-\d{2}-\d{2}T.*(?:Z|[+-]\d{2}:\d{2})$/i.test(value)) {
    // Emby sends fractional seconds with up to seven digits. Date only needs milliseconds.
    const date = new Date(value.replace(/(\.\d{3})\d+/, "$1"));
    if (Number.isFinite(date.getTime())) {
      const year = String(date.getFullYear()).padStart(4, "0");
      const month = String(date.getMonth() + 1).padStart(2, "0");
      const day = String(date.getDate()).padStart(2, "0");
      label = `${year}-${month}-${day}`;
    }
  }
  return <time dateTime={value}>{label}</time>;
}

export function Episodes({ episodes, premiereDates, subjectEpisodes, onError }: {
  episodes: Episode[];
  premiereDates?: ReadonlyMap<string, string>;
  subjectEpisodes?: boolean;
  onError: (message: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const episodeSeason = (episode: Episode) => episode.season ?? (subjectEpisodes ? -1 : 0);
  const seasons = [...new Set(episodes.map(episodeSeason))].sort((a, b) => a - b);
  const [season, setSeason] = useState(seasons.find((value) => value > 0) ?? seasons[0] ?? 0);
  if (!episodes.length) return null;

  return (
    <details className="episodes" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>分集列表 <span>{episodes.length} 集</span></summary>
      {open && (
        <>
          <label className="filter">
            {subjectEpisodes ? "分集类别" : "按季查看"}
            <select className="field" value={season} onChange={(event) => setSeason(Number(event.target.value))}>
              {seasons.map((value) => <option key={value} value={value}>{value === -1 ? "正片" : value === 0 ? "特别篇" : "第 " + value + " 季"}</option>)}
            </select>
          </label>
          <ol className="episode-list">
            {episodes.filter((episode) => episodeSeason(episode) === season).map((episode) => {
              const date = premiereDates?.get(String(episode.id)) ?? episode.date;
              return <li key={episode.id}>
                <span>{episode.number == null ? (season === 0 ? "特别篇" : "—") : "第 " + episode.number + " 集"}</span>
                {episode.url ? <ExternalLink value={episode.url} onError={onError}>{episode.title}</ExternalLink> : episode.title}
                {date && <LocalDate value={date} />}
              </li>;
            })}
          </ol>
        </>
      )}
    </details>
  );
}
