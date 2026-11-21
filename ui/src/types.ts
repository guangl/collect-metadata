export type MediaType = "movie" | "series" | "av";
export type MediaProvider = "wikidata" | "tvmaze" | "tmdb";
export const providerLabels: Record<MediaProvider, string> = {
  wikidata: "Wikidata",
  tvmaze: "TVmaze",
  tmdb: "TMDB",
};
export type StorageStatus = "pending" | "ready" | "failed" | "none";
export const storageLabels: Record<StorageStatus, string> = {
  pending: "正在检查 PostgreSQL",
  ready: "PostgreSQL 已连接",
  failed: "PostgreSQL 连接异常",
  none: "未配置 PostgreSQL",
};
export type TmdbStatus = "unconfigured" | "environment" | "saved";
export type PortraitRole = "actor" | "voice" | "director" | "writer" | "crew";

export interface OutputOptions {
  enabled: boolean;
  directory: string;
  directory_template: string;
  filename_prefix: string;
  metadata: boolean;
  synopsis: boolean;
  credits: boolean;
  ratings: boolean;
  episodes: boolean;
  raw: boolean;
  emby_nfo: boolean;
  poster: boolean;
  backdrop: boolean;
  stills: boolean;
  portraits: PortraitRole[];
}

export interface EmbySettingsView {
  enabled: boolean;
  server: string;
  configured: boolean;
}

export interface EmbyInput {
  enabled: boolean;
  server: string;
  api_key: string;
}

export interface EmbyWorkPreview {
  id: string;
  kind: MediaType;
  title: string;
}

export interface EmbyLibrary {
  id: string;
  name: string;
}

export interface OutputSettingsView {
  root: string | null;
  options: OutputOptions;
}

export interface OutputReport {
  directory: string | null;
  written: number;
  failed: number;
  message: string | null;
}

export interface Episode {
  id: number;
  season: number | null;
  number: number | null;
  title: string;
  description?: string | null;
  date: string | null;
  runtime_minutes: number | null;
  url: string | null;
}

export interface Fields {
  title?: string;
  title_ja?: string;
  title_original?: string;
  title_en?: string;
  title_site?: string;
  dvd_id?: string;
  content_id?: string;
  original_languages?: string[];
  rating?: { value: number; scale: number; count?: number | null };
  episodes?: Episode[];
  cover_url?: string;
  poster_url?: string;
  description?: string;
  [name: string]: unknown;
}

export interface Source {
  provider: string;
  status: "ok" | "partial" | "not_found" | "unavailable";
  note: string | null;
  url: string;
  fields: Fields;
  data: unknown;
}

export interface LookupRecord {
  schema_version: number;
  query: string;
  media_type?: MediaType;
  work_id?: string;
  fields: Fields;
  field_sources: Record<string, string | undefined>;
  sources: Source[];
  metadata_languages?: string[];
  localized_metadata?: Record<string, Record<string, string>>;
}

export interface Snapshot {
  id: number;
  code: string;
  title: string | null;
  media_type: MediaType;
  original_languages: string[];
  collected_at: string;
  cover_url?: string | null;
  year?: number | null;
}

export interface LookupResponse {
  record: LookupRecord;
  saved: Snapshot | null;
  storage_error: string | null;
}

export interface Candidate {
  provider: MediaProvider;
  id: string;
  title: string;
  match_titles?: string[];
  year: number | null;
  original_languages: string[];
  cover_url: string | null;
}

export const mediaLabels: Record<MediaType, string> = {
  movie: "电影",
  series: "剧集",
  av: "AV 番号",
};

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
