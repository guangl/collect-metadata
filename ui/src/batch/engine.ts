import type { Candidate, MediaType, OutputReport } from "../types";

export const MAX_ITEMS = 100;
export const AV_PREFIXES = ["有码番号", "无码番号", "素人番号", "FC2番号", "欧美番号", "国产番号", "动漫里番", "mywife"];
const types = new Map<string, MediaType>([["电影", "movie"], ["剧集", "series"]]);
for (const label of ["番号", "av", ...AV_PREFIXES]) types.set(label.toLowerCase(), "av");

export interface BatchInput {
  kind: MediaType;
  query: string;
}

export const MAX_FOLDER_FILES = 30_000;

export interface FolderPreview {
  works: (BatchInput & { file_count: number })[];
  files: number;
  skipped: number;
  unrecognized: string[];
}

export interface FolderSelection {
  folder: string;
  paths: string[];
  preview: FolderPreview;
}

export interface FolderBatchResult {
  id: number | null;
  works: number;
  files: number;
  skipped: number;
  unrecognized: string[];
}

export type BatchStatus = "running" | "paused" | "cancelled";
export type BatchAction = "pause" | "resume" | "cancel" | "retry_failed";

export interface BatchView {
  status: BatchStatus;
  rows: BatchRow[];
}

export interface BatchSummary {
  id: number;
  created_at: string;
  folder_name: string | null;
  status: BatchStatus;
  total: number;
  finished: number;
  waiting: number;
}

export type BatchStage = "queued" | "searching" | "collecting" | "choice" | "success" | "partial" | "not_found" | "failed" | "paused" | "cancelled";

export interface BatchRow extends BatchInput {
  id: number;
  stage: BatchStage;
  candidates: Candidate[] | null;
  message: string | null;
  has_result: boolean;
  title: string | null;
  cover_url?: string | null;
  original_languages: string[];
  file_count: number;
  output: OutputReport | null;
}

export function parseBatch(input: string, defaultType: MediaType): BatchInput[] {
  const lines = input.split(/\r\n|[\r\n]/).map((line) => line.trim()).filter(Boolean);
  if (!lines.length) throw new Error("请至少输入一条作品名称或番号。");
  if (lines.length > MAX_ITEMS) throw new Error("每批最多 " + MAX_ITEMS + " 条，请分批输入。");

  return lines.map((line) => {
    const prefix = /^([^:：]+)[:：]/.exec(line);
    const kind = prefix && types.get(prefix[1].normalize("NFKC").trim().toLowerCase());
    return kind && prefix
      ? { kind, query: line.slice(prefix[0].length).trim() }
      : { kind: defaultType, query: line };
  });
}

export function terminal(stage: BatchStage): boolean {
  return stage === "success" || stage === "partial" || stage === "not_found" || stage === "failed" || stage === "cancelled";
}
