import { useEffect, useId, useLayoutEffect, useRef, useState, type MouseEvent, type PointerEvent } from "react";
import { getEmbyEpisodeOverview, type EpisodeOverview } from "../../api";
import { LocalDate } from "../../Catalog";
import { Tabs } from "../../Tabs";
import { errorText, mediaLabels, type EmbyEpisodePreview, type EmbySeasonPreview, type EmbyWorkPreview } from "../../types";
import { EmbyArtwork } from "./Artwork";
import { EmbyCredits } from "./Credits";
import { EmbyScrapeActions, type ScrapeControls } from "./ScrapeActions";
import { EmbySeriesEpisodes, episodeLabel, seasonLabel } from "./SeriesEpisodes";

function clickedBackdrop(event: MouseEvent<HTMLDialogElement> | PointerEvent<HTMLDialogElement>) {
  if (event.target !== event.currentTarget) return false;
  const bounds = event.currentTarget.getBoundingClientRect();
  return event.clientX < bounds.left || event.clientX >= bounds.right
    || event.clientY < bounds.top || event.clientY >= bounds.bottom;
}

function EpisodeSynopsis({ episode, revision }: { episode: EmbyEpisodePreview; revision: number }) {
  const [localized, setLocalized] = useState<EpisodeOverview>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    setError(undefined);
    getEmbyEpisodeOverview(episode.id).then(
      (overview) => { if (active) setLocalized(overview); },
      (reason) => {
        if (active) {
          setLocalized(undefined);
          setError(errorText(reason, "EMBY_DETAILS_FAILED"));
        }
      },
    );
    return () => { active = false; };
  }, [episode.id, revision]);

  const overview = localized?.overview ?? episode.overview;
  const source = localized?.source;
  return <>
    <p className="movie-description">{overview || "Emby 暂无本集简介。"}</p>
    {error && <p className="batch-error" role="alert">{error}</p>}
    {!!overview && source && source !== "Emby" && <p className="mt-2 text-sm text-muted">简介来源 · {source}</p>}
  </>;
}

function WorkSynopsis({ work }: { work: EmbyWorkPreview }) {
  const hasDetails = work.official_rating || work.critic_rating != null || work.imdb_rating || work.tmdb_id || work.bangumi_id || !!work.tags?.length;
  return <div className="work-panel work-synopsis">
    <div>
      <h3>剧情简介</h3>
      <p className="movie-description mt-2">{work.overview || "Emby 暂无剧情简介。"}</p>
    </div>
    {hasDetails && <div className="work-details">
      <h3>条目信息</h3>
      <dl>
        {work.official_rating && <div><dt>内容分级</dt><dd>{work.official_rating}</dd></div>}
        {work.critic_rating != null && <div><dt>影评人评分</dt><dd>{work.critic_rating} / 100</dd></div>}
        {work.imdb_rating && <div><dt>IMDb 评分</dt><dd>{work.imdb_rating.value.toFixed(1)} / 10 · {work.imdb_rating.count.toLocaleString("zh-CN")} 人评价</dd></div>}
        {work.tmdb_id && <div><dt>TMDB ID</dt><dd>{work.tmdb_id}</dd></div>}
        {work.bangumi_id && <div><dt>Bangumi ID</dt><dd>{work.bangumi_id}</dd></div>}
        {!!work.tags?.length && <div><dt>标签</dt><dd>
          <ul className="work-chips" role="list">{work.tags.map((tag) => <li key={tag}>{tag}</li>)}</ul>
        </dd></div>}
      </dl>
    </div>}
  </div>;
}

function WorkPictures({ work }: { work: EmbyWorkPreview }) {
  return <div className="work-panel work-pictures">
    <figure>
      <figcaption>海报</figcaption>
      <EmbyArtwork id={work.id} tag={work.poster} image="poster" alt={`《${work.title}》海报`} />
    </figure>
    <figure>
      <figcaption>背景图（Fanart）</figcaption>
      <EmbyArtwork id={work.id} tag={work.backdrop} image="backdrop" alt={`《${work.title}》背景图`} />
    </figure>
    <figure>
      <figcaption>透明标题图（Clearlogo）</figcaption>
      <EmbyArtwork id={work.id} tag={work.logo ?? null} image="logo" alt={`《${work.title}》透明标题图`} />
    </figure>
    <figure>
      <figcaption>横版缩略图（Landscape）</figcaption>
      <EmbyArtwork id={work.id} tag={work.landscape ?? null} image="landscape" alt={`《${work.title}》横版缩略图`} />
    </figure>
  </div>;
}

export function EmbyWorkPreviewDialog({ work, library, missingEpisodeIds, onClose, canScrape, usesQueue, onScrape }: ScrapeControls & {
  work: EmbyWorkPreview;
  library: string;
  missingEpisodeIds?: string[];
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const pressedBackdrop = useRef(false);
  const title = useId();
  const episodeTitle = useId();
  const back = useRef<HTMLButtonElement>(null);
  const origin = useRef<{ button: HTMLButtonElement; scroll: number }>(null);
  const [episodeRevision, setEpisodeRevision] = useState(0);
  const [selected, setSelected] = useState<{ episode: EmbyEpisodePreview; season: EmbySeasonPreview }>();
  useLayoutEffect(() => {
    const frame = dialog.current;
    if (!frame) return;
    const page = document.documentElement;
    const overflow = page.style.overflow;
    frame.showModal();
    page.style.overflow = "hidden";
    return () => { page.style.overflow = overflow; if (frame.open) frame.close(); };
  }, []);
  useLayoutEffect(() => {
    if (!dialog.current) return;
    if (selected) {
      dialog.current.scrollTop = 0;
      back.current?.focus({ preventScroll: true });
    } else if (origin.current) {
      dialog.current.scrollTop = origin.current.scroll;
      origin.current.button.focus({ preventScroll: true });
      origin.current = null;
    }
  }, [selected]);
  const premiere = selected?.episode.premiere_at || selected?.episode.date;

  function close() {
    dialog.current?.close();
    onClose();
  }

  return <dialog className="work-preview" aria-labelledby={selected ? episodeTitle : title} ref={dialog}
    onCancel={(event) => { event.preventDefault(); close(); }}
    onPointerDown={(event) => { pressedBackdrop.current = clickedBackdrop(event); }}
    onClick={(event) => {
      const dismiss = pressedBackdrop.current && clickedBackdrop(event);
      pressedBackdrop.current = false;
      if (dismiss) close();
    }}>
    <div className="sticky top-0 z-10 h-0">
      <button type="button" className="work-preview-close" aria-label="关闭详情" onClick={close}>×</button>
    </div>
    <div hidden={!!selected}>
      <div className="work-hero emby-work-hero">
        <div className="detail-poster"><EmbyArtwork id={work.id} tag={work.poster} image="poster" eager /></div>
        <div className="movie-intro">
          <p className="work-context">媒体库 · {library}</p>
          <h2 id={title}>{work.title}</h2>
          <dl className="work-facts">
            {work.original_title && work.original_title !== work.title &&
              <div className="work-fact-wide"><dt>原名</dt><dd>{work.original_title}</dd></div>}
            <div><dt>作品类型</dt><dd>{mediaLabels[work.kind]}</dd></div>
            {work.year != null && <div><dt>年份</dt><dd>{work.year}</dd></div>}
            {work.runtime_minutes != null && <div><dt>时长</dt><dd>{work.runtime_minutes} 分钟</dd></div>}
            {work.rating != null && <div><dt>社区评分</dt><dd className="work-rating">{work.rating.toFixed(1)} / 10</dd></div>}
            {!!work.genres.length && <div className="work-fact-wide"><dt>题材</dt><dd>
              <ul className="work-chips" role="list">{work.genres.map((genre) => <li key={genre}>{genre}</li>)}</ul>
            </dd></div>}
          </dl>
          <div className="mt-4"><EmbyScrapeActions target={{ type: "item", id: work.id, scope: work.kind }}
            name={work.title} canScrape={canScrape} usesQueue={usesQueue} onScrape={onScrape} /></div>
        </div>
      </div>
      <Tabs label="作品资料" className="work-tabs" initialTab={missingEpisodeIds ? "episodes" : undefined} tabs={[
        { id: "overview", label: "简介", content: <WorkSynopsis work={work} /> },
        ...(work.kind === "series" ? [{ id: "episodes", label: "季与分集", retain: true, content: <EmbySeriesEpisodes id={work.id} missingEpisodeIds={missingEpisodeIds} onOpen={(episode, season, button) => {
          origin.current = { button, scroll: dialog.current?.scrollTop ?? 0 };
          setSelected({ episode, season });
        }} /> }] : []),
        { id: "credits", label: "演职员", retain: true, content: <EmbyCredits id={work.id} /> },
        { id: "pictures", label: "图片", retain: true, content: <WorkPictures work={work} /> },
      ]} />
    </div>
    {selected && <section className="episode-detail px-4 pb-8 pt-4 sm:px-7 sm:pt-6" aria-labelledby={episodeTitle}>
      <button className="episode-back mb-3 me-12" type="button" ref={back} onClick={() => setSelected(undefined)}>← 返回分集</button>
      <p className="work-context">{[work.title, seasonLabel(selected.season), episodeLabel(selected.episode)].join(" · ")}</p>
      <h2 id={episodeTitle}>{selected.episode.title}</h2>
      {selected.episode.original_title && selected.episode.original_title !== selected.episode.title &&
        <p className="work-original">{selected.episode.original_title}</p>}
      <div className="mt-3"><EmbyScrapeActions key={selected.episode.id}
        target={{ type: "item", id: selected.episode.id, scope: "episode" }}
        name={`${work.title} · ${seasonLabel(selected.season)} · ${selected.episode.title}`}
        canScrape={canScrape} usesQueue={usesQueue} onScrape={onScrape}
        onSubmitted={() => setEpisodeRevision((value) => value + 1)} /></div>
      <Tabs label="分集资料" className="mt-4" tabs={[{ id: "details", label: "分集简介", content: <div className="mt-4 grid items-start gap-4 sm:grid-cols-[minmax(0,45fr)_minmax(0,55fr)] sm:gap-5">
        <EmbyArtwork id={selected.episode.id} tag={selected.episode.image} image="episode" eager />
        <div>
          <dl className="episode-facts">
            {premiere && <div><dt>首播</dt><dd><LocalDate value={premiere} /></dd></div>}
            {selected.episode.runtime_minutes != null && <div><dt>时长</dt><dd>{selected.episode.runtime_minutes} 分钟</dd></div>}
            {selected.episode.rating != null && <div><dt>评分</dt><dd className="work-rating">{selected.episode.rating.toFixed(1)} / 10</dd></div>}
          </dl>
          <EpisodeSynopsis key={selected.episode.id} episode={selected.episode} revision={episodeRevision} />
        </div>
      </div> }, { id: "credits", label: "本集演职员", retain: true, content: <EmbyCredits id={selected.episode.id} /> }]} />
    </section>}
  </dialog>;
}
