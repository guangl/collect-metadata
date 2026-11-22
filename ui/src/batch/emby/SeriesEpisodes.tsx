import { useEffect, useId, useState } from "react";
import { getEmbySeries } from "../../api";
import { errorText, type EmbyEpisodePreview, type EmbySeasonPreview, type EmbySeriesPreview } from "../../types";
import { EmbyArtwork } from "./Artwork";

export function seasonLabel(season: EmbySeasonPreview) {
  if (season.number === 0) return "特别篇";
  if (season.number === null) return "未分季";
  return `第 ${season.number} 季`;
}

export function episodeLabel(episode: EmbyEpisodePreview) {
  const { number, last_number } = episode;
  if (number === null) return "未编号";
  return last_number != null && last_number > number ? `第 ${number}–${last_number} 集` : `第 ${number} 集`;
}

export function EmbySeriesEpisodes({ id, missingEpisodeIds, onOpen }: {
  id: string;
  missingEpisodeIds?: string[];
  onOpen: (episode: EmbyEpisodePreview, season: EmbySeasonPreview, button: HTMLButtonElement) => void;
}) {
  const heading = useId();
  const [result, setResult] = useState<EmbySeriesPreview>();
  const [selectedSeason, setSelectedSeason] = useState<number>();
  const [onlyMissing, setOnlyMissing] = useState(!!missingEpisodeIds);
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    setError(undefined);
    setResult(undefined);
    getEmbySeries(id).then(
      (series) => { if (active) setResult(series); },
      (error) => { if (active) setError(errorText(error)); },
    );
    return () => { active = false; };
  }, [id, attempt]);

  const allSeasons = result?.seasons ?? [];
  const missingIds = new Set(missingEpisodeIds);
  const seasons = onlyMissing ? allSeasons.map((season) => ({
    ...season,
    episodes: season.episodes.filter((episode) => missingIds.has(episode.id) && !episode.overview?.trim()),
  })).filter((season) => season.episodes.length > 0) : allSeasons;
  const firstRegular = seasons.findIndex((season) => (season.number ?? 0) > 0 && season.episodes.length > 0);
  const firstAvailable = seasons.findIndex((season) => season.episodes.length > 0);
  const index = selectedSeason ?? (firstRegular >= 0 ? firstRegular : Math.max(firstAvailable, 0));
  const season = seasons[index];
  const episodeCount = seasons.reduce((count, item) => count + item.episodes.length, 0);

  return <section className="series-episodes" aria-labelledby={heading}>
    <div className="section-heading justify-start">
      <h3 id={heading}>季与分集</h3>
      {episodeCount > 0 && <span>{episodeCount} 集</span>}
    </div>
    {error && <p className="batch-error" role="alert">{error}</p>}
    {result?.partial && <p className="muted" role="status">部分分集暂未读取。</p>}
    {(error || result?.partial) && <button className="secondary" type="button" onClick={() => {
      setSelectedSeason(undefined);
      setAttempt((value) => value + 1);
    }}>重新读取分集</button>}
    {!result && !error && <p className="muted" role="status">正在读取 Emby 分集…</p>}
    {result && missingEpisodeIds && <label className="check-option mt-2">
      <input type="checkbox" checked={onlyMissing} onChange={(event) => {
        setSelectedSeason(undefined);
        setOnlyMissing(event.target.checked);
      }} />仅看缺少简介的分集
    </label>}
    {result && !seasons.length && <p className="muted">
      {onlyMissing ? "已读取的分集中没有缺简介的条目。" : "Emby 暂无分集。"}
    </p>}
    {season && <>
      <div className="my-3 flex flex-wrap gap-2" role="group" aria-label="按季查看">
        {seasons.map((item, position) => <button type="button" className="season-button"
          key={item.number ?? "unassigned"} aria-pressed={position === index}
          title={item.title ?? undefined} onClick={() => setSelectedSeason(position)}>{seasonLabel(item)}</button>)}
      </div>
      {season.episodes.length ? <ol className="grid gap-1">
        {season.episodes.map((episode) => <li key={episode.id}>
          <button type="button" className="episode-preview"
            onClick={(event) => onOpen(episode, season, event.currentTarget)}>
            <EmbyArtwork id={episode.id} tag={episode.image} image="episode" />
            <span className="episode-preview-caption">
              <span className="episode-meta">{[episodeLabel(episode), episode.runtime_minutes && `${episode.runtime_minutes} 分钟`].filter(Boolean).join(" · ")}</span>
              <strong>{episode.title}</strong>
            </span>
            <span className="episode-chevron" aria-hidden="true">›</span>
          </button>
        </li>)}
      </ol> : <p className="muted">这季暂时没有分集。</p>}
    </>}
  </section>;
}
