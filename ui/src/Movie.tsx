import { useId, useState } from "react";
import { exportJson } from "./api";
import { Tabs } from "./Tabs";
import { RecordedCredits } from "./RecordedCredits";
import { EmbyCredits } from "./batch/EmbyCatalog";
import { Episodes, ExternalLink, languageName, languageText, PosterArt, safeUrl } from "./Catalog";
import { errorText, mediaLabels } from "./types";
import type { Fields, LookupResponse, MediaType, Source } from "./types";

type Notice = (message: string) => void;

const providerInfo: Record<string, string> = {
  Emby: "家庭媒体库中的作品、图片和现有译文",
  "R18.dev": "日英标题、制作商、演职员和素材",
  JAVDatabase: "发行信息、影片简介和剧照",
  JavDB: "社区评分、演员和站点标题",
  Wikidata: "多语言片名、原始语言、演职员和发行资料",
  TVmaze: "剧集简介、平台、演员、季及分集资料",
  TMDB: "电影和剧集简介、演职员、发行资料、图片与分集",
};
const sourceStatus = {
  ok: "已采集",
  partial: "部分采集",
  not_found: "无匹配",
  unavailable: "暂不可用",
};
const factLabels: Record<string, string> = {
  title: "片名",
  original_languages: "原始语言",
  end_date: "完结日期",
  countries: "制作国家或地区",
  cast: "演员",
  writers: "编剧",
  producers: "制片人",
  network: "播出平台",
  status: "播出状态",
  season_number: "季号",
  episode_number: "集号",
  episode_last_number: "结束集号",
  season_count: "季数",
  episode_count: "集数",
  imdb_id: "IMDb ID",
  tmdb_id: "TMDB ID",
  tagline: "宣传语",
  official_url: "官方网站",
  watch_providers_source: "播放平台数据来源",
  dvd_id: "番号",
  content_id: "商品 ID",
  release_date: "发行日期",
  runtime_minutes: "时长",
  rating: "评分",
  studio: "制作商",
  label: "厂牌 / 发行",
  series: "系列",
  directors: "导演",
  voice_actors: "声优",
  actresses: "女演员",
  actors: "男演员",
  other_cast: "其他演员",
  title_ja: "日文片名",
  title_original: "来源原片名",
  title_en: "英文片名",
  title_site: "JavDB 站点标题",
  genres: "分类",
  description: "影片简介",
  cover_url: "封面",
  poster_url: "海报",
  backdrop_url: "背景图",
  trailer_url: "预告片",
};
const keyFacts = [
  "original_languages", "content_id", "release_date", "end_date", "runtime_minutes", "rating",
  "countries", "studio", "label", "series", "network", "status", "season_number", "episode_number", "episode_last_number", "season_count", "episode_count", "imdb_id", "tmdb_id",
];
const additionalFacts = [
  "title_ja", "title_original", "title_en", "title_site", "genres", "tagline",
  "cover_url", "poster_url", "backdrop_url", "trailer_url", "official_url",
];

export function formatDate(value: string): string {
  try {
    return new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
  } catch {
    return value;
  }
}

function FactList({ fields, origins, keys = Object.keys(fields), className = "facts", onError }: {
  fields: Fields;
  origins?: Record<string, string | undefined>;
  keys?: string[];
  className?: string;
  onError: Notice;
}) {
  const presentKeys = keys.filter((key) => key !== "episodes" && fields[key] != null);
  if (!presentKeys.length) return null;

  return (
    <dl className={className}>
      {presentKeys.map((key) => (
        <div className={className === "facts" ? "fact" : undefined} key={key}>
          <dt>{factLabels[key] || key}{origins?.[key] && <span>{origins[key]}</span>}</dt>
          <dd><FactValue name={key} value={fields[key]} onError={onError} /></dd>
        </div>
      ))}
    </dl>
  );
}

function FactValue({ name, value, onError }: { name: string; value: unknown; onError: Notice }) {
  if (name.endsWith("_url") && typeof value === "string") {
    return <ExternalLink value={value} onError={onError}>打开链接 ↗</ExternalLink>;
  }
  if (name === "rating" && value && typeof value === "object" && "value" in value && "scale" in value) {
    const count = "count" in value && typeof value.count === "number" ? " · " + value.count + " 人评价" : "";
    return <>{String(value.value)} / {String(value.scale)}{count}</>;
  }
  if (name === "runtime_minutes") return <>{String(value)} 分钟</>;
  if (Array.isArray(value)) return name === "original_languages" ? languageText(value) : value.join(" · ");
  return String(value);
}

function SourceCard({ name, description, source, loading, onError }: {
  name: string;
  description: string;
  source?: Source;
  loading: boolean;
  onError: Notice;
}) {
  const [showRaw, setShowRaw] = useState(false);
  const heading = (
    <>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3>{name}</h3>
        {(source || loading) && <span className="source-state" data-status={source?.status}>
          {source ? sourceStatus[source.status] : "采集中"}
        </span>}
      </div>
      <p>{source?.note || description}</p>
    </>
  );
  if (!source) return <article className="source-card">{heading}</article>;
  return (
    <details className="source-card">
      <summary>{heading}</summary>
      <ExternalLink value={source.url} className="source-link" onError={onError}>打开来源页面 ↗</ExternalLink>
      <FactList fields={source.fields} onError={onError} />
      {source.data != null && <details onToggle={(event) => setShowRaw(event.currentTarget.open)}>
        <summary className="raw">查看来源 JSON</summary>
        {showRaw && <pre tabIndex={0}>{JSON.stringify(source.data, null, 2)}</pre>}
      </details>}
    </details>
  );
}

export function Sources({ kind, sources, loading, onError }: {
  kind: MediaType;
  sources?: Source[];
  loading: boolean;
  onError: Notice;
}) {
  const titleId = useId();
  const names = sources?.map(({ provider }) => provider) ??
    (kind === "av" ? ["R18.dev", "JAVDatabase", "JavDB"] : kind === "movie" ? ["Wikidata"] : ["Wikidata", "TVmaze"]);
  const successes = sources?.filter((item) => ["ok", "partial"].includes(item.status)).length;
  return (
    <section aria-labelledby={titleId}>
      <div className="section-heading">
        <h2 id={titleId}>数据来源</h2>
        {sources && <span>{successes} / {sources.length} 个来源有结果</span>}
      </div>
      <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-3">
        {names.map((name) => (
          <SourceCard name={name} description={providerInfo[name]} key={name}
            source={sources?.find((item) => item.provider === name)} loading={loading} onError={onError} />
        ))}
      </div>
    </section>
  );
}

export function Movie({ response, onNotice, onError }: { response: LookupResponse; onNotice: Notice; onError: Notice }) {
  const [exporting, setExporting] = useState(false);
  const { record, saved } = response;
  const { fields: baseFields, field_sources, sources, metadata_languages, localized_metadata } = record;
  const primary = metadata_languages?.[0];
  const [selection, setSelection] = useState<{ record: typeof record; language: string }>();
  const language = selection?.record === record ? selection.language : primary;
  const variant = language && language !== primary ? localized_metadata?.[language] : undefined;
  const fields = { ...baseFields };
  const origins = { ...field_sources };
  if (variant) {
    for (const key of ["title", "description", "tagline"] as const) {
      if (variant[key]) {
        fields[key] = variant[key];
        delete origins[key];
      }
    }
    fields.episodes = fields.episodes?.map((episode, index) => ({
      ...episode,
      title: variant[`episodes/${index}/title`] ?? episode.title,
      description: variant[`episodes/${index}/description`] ?? episode.description,
    }));
  }
  if (!Object.keys(fields).length) {
    const unavailable = sources.some((source) => source.status === "unavailable");
    return (
      <section className="panel no-match" aria-label="采集结果">
        <h1>暂无 {record.query} 的资料</h1>
        <p>{unavailable ? "部分来源未能访问。可展开下方来源查看详情。" : "请核对番号或商品 ID。"}</p>
        <Sources kind={record.media_type ?? "av"} sources={sources} loading={false} onError={onError} />
      </section>
    );
  }

  const code = fields.dvd_id || fields.content_id || record.work_id || record.query;
  const title = fields.title || fields.title_ja || fields.title_original || fields.title_en || fields.title_site;
  const kind = record.media_type ?? "av";
  const backdrop = safeUrl(fields.backdrop_url);
  const embyItem = (sources.find(({ provider }) => provider === "Emby")?.data as { item?: { Id?: unknown } } | undefined)?.item?.Id;
  const liveCredits = typeof embyItem === "string" && /^[a-z0-9-]{1,96}$/i.test(embyItem) ? embyItem : undefined;
  async function save() {
    setExporting(true);
    try {
      if (await exportJson(response, code)) onNotice("已导出 JSON。");
    } catch (error) {
      onError(errorText(error));
    } finally {
      setExporting(false);
    }
  }

  return (
    <article className="movie" aria-label="采集结果">
      <div className="work-hero">
        {backdrop && <span className="work-backdrop" aria-hidden="true">
          <img src={backdrop} alt="" decoding="async" referrerPolicy="no-referrer" />
        </span>}
        <div className="detail-poster"><PosterArt url={fields.poster_url || fields.cover_url} /></div>
        <div className="movie-intro">
          <p className="movie-code">{kind === "av" ? code : (record.scope === "episode" ? "分集" : mediaLabels[kind]) + " · " + code}</p>
          <h1>{title ?? "影片资料"}</h1>
          {metadata_languages && metadata_languages.length > 1 && <label className="batch-input">元数据语言
            <select className="field" value={language} onChange={(event) => setSelection({ record, language: event.target.value })}>
              {metadata_languages.map((code) => <option key={code} value={code}>{languageName(code)}</option>)}
            </select>
          </label>}
          {language && language !== primary && <p className="muted">该语言缺少的字段沿用首选语言或原文。</p>}
          {saved && <p className="date-note">采集于 {formatDate(saved.collected_at)}</p>}
          <button type="button" className="secondary" onClick={save} disabled={exporting}>导出 JSON</button>
        </div>
      </div>
      <Tabs label="采集资料" className="result-tabs" tabs={[
        { id: "overview", label: "简介与资料", content: <div className="movie-body">
          {fields.description && <p className="movie-description mb-4">{fields.description}</p>}
          <FactList fields={{ original_languages: [], ...fields }} origins={origins} keys={keyFacts} onError={onError} />
          <FactList fields={fields} origins={origins} keys={additionalFacts} className="full-facts" onError={onError} />
        </div> },
        ...(fields.episodes?.length ? [{ id: "episodes", label: "季与分集", retain: true, content: <div className="movie-body"><Episodes episodes={fields.episodes} onError={onError} /></div> }] : []),
        { id: "credits", label: "演职员", retain: true, content: liveCredits ? <EmbyCredits id={liveCredits} fallback={<RecordedCredits sources={sources} fields={fields} />} /> : <RecordedCredits sources={sources} fields={fields} /> },
        { id: "sources", label: "数据来源", retain: true, content: <div className="movie-body"><Sources kind={kind} sources={sources} loading={false} onError={onError} /></div> },
      ]} />
    </article>
  );
}
