import { memo, useEffect, useRef, useState } from "react";
import { getMissingEpisodeOverviews } from "../../api";
import { Tabs } from "../../Tabs";
import { errorText, mediaLabels, type EmbyLibraryPreview, type EmbyWorkPreview } from "../../types";
import { EmbyArtwork } from "./Artwork";
import { EmbyScrapeActions, type ScrapeControls } from "./ScrapeActions";
import { EmbyScrapeDialog, type EmbyScrapeChoice } from "./ScrapeDialog";
import { EmbyWorkPreviewDialog } from "./WorkPreview";

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
          {!selecting && <EmbyScrapeActions target={{ type: "library", id: library.id }} name={library.name}
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

export const EmbyCatalog = memo(function EmbyCatalog({ libraries, onOpenBatch, ...controls }: ScrapeControls & SelectionControls & {
  libraries: EmbyLibraryPreview[];
  onOpenBatch: (id: number) => void;
}) {
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
    {selected && <EmbyWorkPreviewDialog {...selected} canScrape={controls.canScrape} usesQueue={controls.usesQueue} onScrape={controls.onScrape}
      onOpenBatch={onOpenBatch} onClose={() => setSelected(undefined)} />}
  </div>;
});
