import { memo, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent, PointerEvent, ReactNode } from "react";
import { getEmbyCredits, getEmbyEpisodeOverview, getEmbySeries, getMissingEpisodeOverviews, isDesktop, loadEmbyImage, overwriteEmby } from "../api";
import { LocalDate } from "../Catalog";
import { Tabs } from "../Tabs";
import { EmbyScrapeDialog, type EmbyScrapeChoice } from "./EmbyScrapeDialog";
import type { EmbyImage, EpisodeOverview } from "../api";
import { errorText, mediaLabels } from "../types";
import type { EmbyCredit, EmbyEpisodePreview, EmbyLibraryPreview, EmbyScrapeTarget, EmbySeasonPreview, EmbySeriesPreview, EmbyWorkPreview } from "../types";

const artworkClasses: Record<EmbyImage, string> = {
  poster: "poster-art",
  backdrop: "work-backdrop",
  episode: "episode-art",
  portrait: "credit-art",
};

const imageQueue: (() => Promise<void>)[] = [];
let loadingImages = 0;

function loadNextImage() {
  if (loadingImages >= 6) return;
  const task = imageQueue.shift();
  if (!task) return;
  loadingImages++;
  void task().finally(() => {
    loadingImages--;
    loadNextImage();
  });
}

function EmbyArtwork({ id, tag, image, eager = false }: {
  id: string;
  tag: string | null;
  image: EmbyImage;
  eager?: boolean;
}) {
  const host = useRef<HTMLSpanElement>(null);
  const imageKey = `${id}/${image}?v=${tag}`;
  const [local, setLocal] = useState<{ key: string; url?: string }>();
  const [loaded, setLoaded] = useState<string>();
  const [failed, setFailed] = useState<string>();
  const src = !tag ? undefined : isDesktop ? (local?.key === imageKey ? local.url : undefined)
    : `/api/emby/images/${encodeURIComponent(id)}/${image}?v=${encodeURIComponent(tag)}`;

  useEffect(() => {
    if (!isDesktop || !tag || !host.current) return;
    let stopped = false;
    let url: string | undefined;
    let observer: IntersectionObserver | undefined;
    async function load() {
      try {
        url = await loadEmbyImage(id, image);
        if (stopped) URL.revokeObjectURL(url);
        else setLocal({ key: imageKey, url });
      } catch {
        if (!stopped) setLocal({ key: imageKey });
      }
    }
    function enqueue() {
      if (eager) imageQueue.unshift(load);
      else imageQueue.push(load);
      loadNextImage();
    }

    if (eager || typeof IntersectionObserver === "undefined") {
      enqueue();
    } else {
      observer = new IntersectionObserver(([entry]) => {
        if (entry.isIntersecting) {
          observer?.disconnect();
          enqueue();
        }
      }, { rootMargin: "240px" });
      observer.observe(host.current);
    }
    return () => {
      stopped = true;
      observer?.disconnect();
      const queued = imageQueue.indexOf(load);
      if (queued >= 0) imageQueue.splice(queued, 1);
      if (url) URL.revokeObjectURL(url);
    };
  }, [id, image, tag, eager, imageKey]);

  const unavailable = (local?.key === imageKey && !local.url) || (!!src && failed === src);
  const emptyLabel = image === "poster" ? "无海报" : image === "portrait" ? "无头像" : "无剧照";
  return <span ref={host} className={artworkClasses[image]} aria-hidden="true">
    {image !== "backdrop" && (!src || loaded !== src) && <span className="poster-placeholder">
      {!tag ? emptyLabel : unavailable ? "图片加载失败" : "正在加载图片…"}
    </span>}
    {src && failed !== src && <img src={src} alt="" loading={eager || isDesktop ? "eager" : "lazy"} decoding="async"
      className={loaded === src ? undefined : "opacity-0"} onLoad={() => setLoaded(src)} onError={() => setFailed(src)} />}
  </span>;
}

function seasonLabel(season: EmbySeasonPreview) {
  if (season.number === 0) return "特别篇";
  if (season.number === null) return "未分季";
  return `第 ${season.number} 季`;
}

function episodeLabel(episode: EmbyEpisodePreview) {
  const { number, last_number } = episode;
  if (number === null) return "未编号";
  return last_number != null && last_number > number ? `第 ${number}–${last_number} 集` : `第 ${number} 集`;
}

function SeriesEpisodes({ id, missingEpisodeIds, onOpen }: {
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

const creditRoles: [EmbyCredit["role"], string][] = [
  ["voice", "声优"], ["actor", "演员"], ["director", "导演"], ["writer", "编剧"], ["producer", "制片人"], ["crew", "其他幕后人员"],
];

export function EmbyCredits({ id, fallback }: { id: string; fallback?: ReactNode }) {
  const [people, setPeople] = useState<EmbyCredit[]>();
  const [error, setError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setError(undefined);
    setPeople(undefined);
    getEmbyCredits(id).then((credits) => { if (active) setPeople(credits); }, (reason) => { if (active) setError(errorText(reason)); });
    return () => { active = false; };
  }, [id, attempt]);
  if (error) return <><div className="work-panel"><p className="batch-error" role="alert">{error}</p>
    <button className="secondary" type="button" onClick={() => setAttempt((count) => count + 1)}>重新读取演职员</button></div>{fallback}</>;
  if (!people) return <p className="work-panel muted" role="status">正在读取演职员…</p>;
  if (!people.length) return fallback ?? <p className="work-panel muted">Emby 未提供该条目的演职员资料。</p>;
  return <div className="work-panel grid gap-5">
    {creditRoles.map(([role, label]) => {
      const group = people.filter((person) => person.role === role);
      return group.length > 0 && <section key={role} aria-label={label}>
        <h3 className="mb-2 text-base">{label}</h3>
        <ul className="credit-grid grid grid-cols-credits gap-2.5 sm:grid-cols-credits-wide sm:gap-3" role="list">{group.map((person, index) => <li key={`${person.id ?? person.name}-${index}`}>
          <EmbyArtwork id={person.id ?? ""} tag={person.id ? person.portrait : null} image="portrait" />
          <strong>{person.name}</strong>{person.detail && <span>{person.detail}</span>}
        </li>)}</ul>
      </section>;
    })}
  </div>;
}

type ScrapeControls = {
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

function ScrapeActions({ target, name, canScrape, usesQueue, onScrape, onSubmitted }: ScrapeControls & {
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

function clickedBackdrop(event: MouseEvent<HTMLDialogElement> | PointerEvent<HTMLDialogElement>) {
  if (event.target !== event.currentTarget) return false;
  const bounds = event.currentTarget.getBoundingClientRect();
  return event.clientX < bounds.left || event.clientX >= bounds.right
    || event.clientY < bounds.top || event.clientY >= bounds.bottom;
}

function EpisodeSynopsis({ episode, revision }: { episode: EmbyEpisodePreview; revision: number }) {
  const [localized, setLocalized] = useState<EpisodeOverview>();
  useEffect(() => {
    let active = true;
    getEmbyEpisodeOverview(episode.id).then(
      (overview) => { if (active) setLocalized(overview); },
      () => { if (active) setLocalized(undefined); },
    );
    return () => { active = false; };
  }, [episode.id, revision]);

  const overview = localized?.overview ?? episode.overview;
  const source = localized?.source;
  return <>
    <p className="movie-description">{overview || "Emby 暂无本集简介。"}</p>
    {!!overview && source && source !== "Emby" && <p className="mt-2 text-sm text-muted">简介来源 · {source}</p>}
  </>;
}

function WorkPreview({ work, library, missingEpisodeIds, onClose, canScrape, usesQueue, onScrape }: ScrapeControls & {
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
  const facts = [mediaLabels[work.kind], work.year, work.runtime_minutes && `${work.runtime_minutes} 分钟`]
    .filter(Boolean).join(" · ");
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
      <div className="work-hero">
        {work.backdrop && <EmbyArtwork id={work.id} tag={work.backdrop} image="backdrop" eager />}
        <div className="detail-poster"><EmbyArtwork id={work.id} tag={work.poster} image="poster" eager /></div>
        <div className="movie-intro">
          <p className="work-context">{library}</p>
          <h2 id={title}>{work.title}</h2>
          {work.original_title && work.original_title !== work.title && <p className="work-original">{work.original_title}</p>}
          <div className="work-meta">
            <span>{facts}</span>
            {work.rating != null && <span className="work-rating" aria-label={`评分 ${work.rating}，满分 10`}>★ {work.rating.toFixed(1)}</span>}
          </div>
          {!!work.genres.length && <p className="work-genres">{work.genres.join(" / ")}</p>}
          <div className="mt-3"><ScrapeActions target={{ type: "item", id: work.id, scope: work.kind }}
            name={work.title} canScrape={canScrape} usesQueue={usesQueue} onScrape={onScrape} /></div>
        </div>
      </div>
      <Tabs label="作品资料" className="work-tabs" initialTab={missingEpisodeIds ? "episodes" : undefined} tabs={[
        { id: "overview", label: "简介", content: <div className="work-panel">
          <p className="movie-description">{work.overview || "Emby 暂无剧情简介。"}</p>
          {(work.official_rating || work.critic_rating != null || work.imdb_rating || work.bangumi_id || !!work.tags?.length) &&
            <dl className="episode-facts mt-4">
              {work.official_rating && <div><dt>分级</dt><dd>{work.official_rating}</dd></div>}
              {work.critic_rating != null && <div><dt>影评人评分</dt><dd>{work.critic_rating} / 100</dd></div>}
              {work.imdb_rating && <div><dt>IMDb 评分</dt><dd>{work.imdb_rating.value.toFixed(1)} / 10 · {work.imdb_rating.count.toLocaleString("zh-CN")} 人评价</dd></div>}
              {work.bangumi_id && <div><dt>Bangumi ID</dt><dd>{work.bangumi_id}</dd></div>}
              {!!work.tags?.length && <div><dt>标签</dt><dd>{work.tags.join(" / ")}</dd></div>}
            </dl>}
        </div> },
        ...(work.kind === "series" ? [{ id: "episodes", label: "季与分集", retain: true, content: <SeriesEpisodes id={work.id} missingEpisodeIds={missingEpisodeIds} onOpen={(episode, season, button) => {
          origin.current = { button, scroll: dialog.current?.scrollTop ?? 0 };
          setSelected({ episode, season });
        }} /> }] : []),
        { id: "credits", label: "演职员", retain: true, content: <EmbyCredits id={work.id} /> },
      ]} />
    </div>
    {selected && <section className="episode-detail px-4 pb-8 pt-4 sm:px-7 sm:pt-6" aria-labelledby={episodeTitle}>
      <button className="episode-back mb-3 me-12" type="button" ref={back} onClick={() => setSelected(undefined)}>← 返回分集</button>
      <p className="work-context">{[work.title, seasonLabel(selected.season), episodeLabel(selected.episode)].join(" · ")}</p>
      <h2 id={episodeTitle}>{selected.episode.title}</h2>
      {selected.episode.original_title && selected.episode.original_title !== selected.episode.title &&
        <p className="work-original">{selected.episode.original_title}</p>}
      <div className="mt-3"><ScrapeActions key={selected.episode.id}
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

type SelectionControls = {
  aiConfigured: boolean;
  onSelection: (works: EmbyWorkPreview[], choice: EmbyScrapeChoice) => Promise<{ submitted: string[]; error?: string }>;
};

const MAX_SELECTION = 500;

function LibraryWorks({ library, canScrape, usesQueue, onScrape, aiConfigured, onSelection, onOpen }: ScrapeControls & SelectionControls & {
  library: EmbyLibraryPreview;
  onOpen: (work: EmbyWorkPreview, missingEpisodeIds?: string[]) => void;
}) {
  const [query, setQuery] = useState("");
  const [missing, setMissing] = useState("all");
  const [selecting, setSelecting] = useState(false);
  const [selections, setSelections] = useState<Set<string>>(() => new Set());
  const [opener, setOpener] = useState<HTMLButtonElement>();
  const toggle = useRef<HTMLButtonElement>(null);
  const [error, setError] = useState("");
  const [episodeFilter, setEpisodeFilter] = useState<{ episodes: Map<string, string[]> } | { error: string }>();
  const [filterAttempt, setFilterAttempt] = useState(0);
  const byEpisode = missing === "episode-overview";
  const missingEpisodes = byEpisode && episodeFilter && "episodes" in episodeFilter ? episodeFilter.episodes : undefined;

  useEffect(() => {
    if (!byEpisode) return;
    let active = true;
    getMissingEpisodeOverviews(library.id).then(
      (result) => { if (active) setEpisodeFilter({ episodes: new Map(Object.entries(result)) }); },
      (error) => { if (active) setEpisodeFilter({ error: errorText(error) }); },
    );
    return () => { active = false; };
  }, [byEpisode, library.id, filterAttempt]);

  const term = query.trim().normalize("NFKC").toLowerCase();
  const visible = library.works.filter((work) =>
    (!term || [work.title, work.original_title, work.year].some((value) =>
      String(value ?? "").normalize("NFKC").toLowerCase().includes(term))) &&
    (missing === "all" || (missing === "poster" && !work.poster) ||
      (missing === "overview" && !work.overview?.trim()) ||
      (byEpisode && work.kind === "series" && !!missingEpisodes?.get(work.id)?.length) ||
      (missing === "any" && (!work.poster || !work.overview?.trim())))
  );
  const selected = library.works.filter((work) => selections.has(work.id));
  const allVisible = visible.length > 0 && visible.every((work) => selections.has(work.id));

  function selectVisible() {
    setSelections((before) => {
      const next = new Set(before);
      for (const work of visible) {
        if (allVisible) next.delete(work.id);
        else if (next.size < MAX_SELECTION) next.add(work.id);
      }
      return next;
    });
  }

  async function submit(choice: EmbyScrapeChoice) {
    const { submitted, error } = await onSelection(selected, choice);
    setSelections((previous) => {
      const next = new Set(previous);
      for (const id of submitted) next.delete(id);
      return next;
    });
    if (error) setError(error);
    else setSelecting(false);
  }

  function toggleSelection() {
    if (selecting) {
      setSelections(new Set());
      setError("");
    }
    setSelecting(!selecting);
  }

  return <section className="py-4" aria-labelledby={`emby-library-${library.id}`}>
    <h3 id={`emby-library-${library.id}`} className="sr-only">{library.name}</h3>
    {library.works.length > 0 && <>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <label className="filter grow sm:grow-0">
          <span className="sr-only">搜索片名</span>
          <input className="field min-w-0" type="search" placeholder="搜索片名" value={query}
            onChange={(event) => setQuery(event.target.value)} />
        </label>
        <label>
          <span className="sr-only">资料完整性</span>
          <select className="field" value={missing} onChange={(event) => {
            setEpisodeFilter(undefined);
            setMissing(event.target.value);
          }}>
            <option value="all">全部作品</option>
            <option value="any">作品缺少海报或简介</option>
            <option value="poster">缺少海报</option>
            <option value="overview">作品缺少简介</option>
            {library.works.some((work) => work.kind === "series") && <option value="episode-overview">分集缺少简介</option>}
          </select>
        </label>
        <div className="flex flex-wrap items-center gap-2 sm:ms-auto">
          <button className="secondary" type="button" ref={toggle} aria-pressed={selecting} disabled={!canScrape}
            onClick={toggleSelection}>{selecting ? "退出选择" : "选择"}</button>
          {!selecting && <ScrapeActions target={{ type: "library", id: library.id }} name={library.name}
            canScrape={canScrape} usesQueue={usesQueue} onScrape={onScrape} />}
        </div>
        {!selecting && (term || missing !== "all") && (!byEpisode || missingEpisodes) &&
          <span className="muted" role="status">显示 {visible.length} 部</span>}
      </div>
      {selecting && <div className="mb-4 flex flex-wrap items-center gap-2">
        <span className="muted" role="status">{(!byEpisode || missingEpisodes) && `显示 ${visible.length} 部 · `}已选 {selected.length} 部</span>
        <button className="secondary" type="button"
          disabled={!canScrape || visible.length === 0 || (!allVisible && selected.length >= MAX_SELECTION)}
          onClick={selectVisible}>{allVisible ? "取消选择当前结果" : "选择当前结果"}</button>
        {selected.length > 0 && <button className="secondary" type="button" disabled={!canScrape}
          onClick={() => setSelections(new Set())}>清空选择</button>}
        <button className="primary" type="button" aria-haspopup="dialog" disabled={!canScrape || !selected.length}
          onClick={(event) => { setError(""); setOpener(event.currentTarget); }}>刮削所选</button>
        {selected.length === MAX_SELECTION && <span className="muted">每批最多选择 {MAX_SELECTION} 部作品。</span>}
      </div>}
      {opener && <EmbyScrapeDialog opener={opener} fallbackFocus={toggle.current} target={`已选 ${selected.length} 部作品`}
        usesQueue={usesQueue} allowAi={aiConfigured}
        overwrite="Emby 服务器调用该媒体库的元数据下载器，重新刮削所选作品。所选剧集包含季和分集。已有元数据会被替换。"
        onClose={() => setOpener(undefined)} onSubmit={submit} />}
      {error && <p className="batch-error" role="alert">{error}</p>}
    </>}
    {byEpisode && !episodeFilter && <p className="muted" role="status">正在检查分集简介…</p>}
    {byEpisode && episodeFilter && "error" in episodeFilter && <div className="flex flex-wrap items-center gap-2">
      <p className="batch-error" role="alert">{episodeFilter.error}</p>
      <button className="secondary" type="button" onClick={() => {
        setEpisodeFilter(undefined);
        setFilterAttempt((value) => value + 1);
      }}>重试</button>
    </div>}
    {visible.length ? <ul className="grid grid-cols-library gap-x-3 gap-y-4 sm:grid-cols-library-wide sm:gap-4" role="list">
      {visible.map((work) => {
        const checked = selections.has(work.id);
        const missingEpisodeIds = missingEpisodes?.get(work.id);
        const missingLabel = missingEpisodeIds?.length ? `缺 ${missingEpisodeIds.length} 集简介` : undefined;
        return <li className="emby-tile relative min-w-0" data-selected={selecting && checked} key={work.id}>
          {selecting ? <label className="absolute inset-0 z-10 cursor-pointer">
            <span className="absolute end-1.5 top-1.5 flex rounded bg-canvas p-1.5 shadow">
              <input type="checkbox" aria-label={`选择 ${work.title}`} checked={checked}
                disabled={!canScrape || (selected.length >= MAX_SELECTION && !checked)}
                onChange={(event) => {
                  const checked = event.target.checked;
                  setSelections((before) => {
                    const next = new Set(before);
                    if (checked) next.add(work.id);
                    else next.delete(work.id);
                    return next;
                  });
                }} />
            </span>
          </label> : <button type="button" className="absolute inset-0 z-10 rounded-lg"
            aria-label={[work.title, work.year, mediaLabels[work.kind], missingLabel].filter(Boolean).join(" · ")}
            title={work.title} aria-haspopup="dialog" onClick={() => onOpen(work, missingEpisodeIds)} />}
          <EmbyArtwork id={work.id} tag={work.poster} image="poster" />
          <span className="emby-tile-caption" aria-hidden="true">
            <strong>{work.title}</strong>
            <span>{[work.year, mediaLabels[work.kind]].filter(Boolean).join(" · ")}</span>
            {missingLabel && <span className="text-accent">{missingLabel}</span>}
          </span>
        </li>;
      })}
    </ul> : (!byEpisode || missingEpisodes) && <p className="muted">{
      library.works.length ? "没有符合筛选条件的作品。" : "这个媒体库没有可刮削的电影、剧集或私人影片。"
    }</p>}
  </section>;
}

export const EmbyCatalog = memo(function EmbyCatalog({ libraries, ...controls }: ScrapeControls & SelectionControls & { libraries: EmbyLibraryPreview[] }) {
  const [selected, setSelected] = useState<{ work: EmbyWorkPreview; library: string; missingEpisodeIds?: string[] }>();

  return <div className="min-w-0">
    <Tabs label="Emby 媒体库" className="mt-3" tabs={libraries.map((library) => ({
      id: library.id,
      label: library.name,
      retain: true,
      panelFocusable: library.works.length === 0,
      content: <LibraryWorks library={library} {...controls}
        onOpen={(work, missingEpisodeIds) => setSelected({ work, library: library.name, missingEpisodeIds })} />,
    }))} />
    {selected && <WorkPreview {...selected} canScrape={controls.canScrape} usesQueue={controls.usesQueue} onScrape={controls.onScrape}
      onClose={() => setSelected(undefined)} />}
  </div>;
});
