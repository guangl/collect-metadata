import type { BatchAction, BatchInput, BatchSummary, BatchView, FolderBatchResult, FolderPreview, FolderSelection } from "./batch/engine";
import type {
  Candidate,
  EmbyInput,
  EmbyLibraryPreview,
  EmbyScrapeScope,
  EmbyScrapeTarget,
  EmbySeriesPreview,
  EmbyCredit,
  EmbySettingsView,
  LookupResponse,
  MediaProvider,
  MediaType,
  OutputOptions,
  OutputSettingsView,
  Snapshot,
  StorageStatus,
  TmdbStatus,
  WritebackPreview,
} from "./types";

declare global {
  interface Window {
    __TAURI__?: {
      core?: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
    };
  }
}

const invoke = window.__TAURI__?.core?.invoke;
export const isDesktop = Boolean(invoke);

function httpHint(status: number): string {
  if (status === 401 || status === 403) return "请确认登录状态和访问权限。";
  if (status === 404 || status === 405) return "请检查反向代理路径，并确认页面与服务端版本一致。";
  if (status === 413) return "请缩小本次提交范围，或检查反向代理的请求大小限制。";
  if (status === 429) return "请求过于频繁，请稍后重试。";
  if (status >= 500) return "请检查 Web 服务、反向代理及应用日志。";
  return "请刷新页面后重试，仍失败时查看应用日志。";
}

async function request<T>(route: string, options?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await fetch(route, options);
  } catch {
    throw new Error("无法连接 Collect Metadata Web。请确认服务在线、网络可达，恢复后重新操作。");
  }
  const id = response.headers.get("x-request-id") ?? "";
  const reference = /^[a-f0-9]{32}$/.test(id) ? `（诊断编号 ${id}）` : "";
  if (!response.ok) {
    const result = await response.json().catch(() => null);
    const message = typeof result?.error === "string" && result.error.trim()
      ? result.error
      : `服务请求失败（HTTP ${response.status}）。${httpHint(response.status)}`;
    throw new Error(message + reference);
  }
  if (response.status === 204) return undefined as T;
  try {
    return await response.json();
  } catch {
    throw new Error("Web 返回了无效的响应。请检查反向代理是否转到了登录页，并确认服务端版本。" + reference);
  }
}

export function desktopLogDirectory(): Promise<string> {
  return invoke!("desktop_log_directory");
}

export function openDesktopLogs(): Promise<void> {
  return invoke!("open_desktop_logs");
}

function post<T>(path: string, data: unknown) {
  return request<T>(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(data),
  });
}

export function getHistory(
  before: number | null,
  kind: MediaType | null,
  language: string | null,
): Promise<Snapshot[]> {
  if (invoke) return invoke("history", { before, kind, language });
  const params = new URLSearchParams();
  if (before !== null) params.set("before", String(before));
  if (kind !== null) params.set("kind", kind);
  if (language !== null) params.set("language", language);
  return request("/api/history?" + params);
}

export function getHistoryLanguages(): Promise<string[]> {
  return invoke ? invoke("history_languages") : request("/api/history/languages");
}

export function loadSnapshot(id: number): Promise<LookupResponse> {
  return invoke
    ? invoke("load_snapshot", { id })
    : request("/api/history/" + encodeURIComponent(id));
}

export async function getStorageStatus(): Promise<StorageStatus> {
  if (invoke) {
    if (!(await invoke<boolean>("storage_configured"))) return "none";
    await invoke("check_storage");
  } else {
    await request("/api/settings/postgres", { cache: "no-store" });
  }
  return "ready";
}

export async function openUrl(url: string): Promise<void> {
  if (invoke) await invoke("plugin:opener|open_url", { url });
}

export async function exportJson(response: LookupResponse, code: string): Promise<boolean> {
  const data = { ...response.record, ...(response.saved && { snapshot: response.saved }) };
  if (invoke) return invoke("export_json", { code, document: data });

  const blob = new Blob([JSON.stringify(data, null, 2), "\n"], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.download = code.replace(/[^a-z0-9-]/gi, "-") + ".json";
  link.href = url;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  return true;
}

export function configureDatabase(url: string): Promise<void> {
  if (!invoke) return Promise.reject(new Error("请在服务端配置 PostgreSQL。"));
  return invoke("configure_database", { url });
}

export function getTmdbStatus(): Promise<TmdbStatus> {
  return invoke ? invoke("tmdb_status") : request("/api/settings/tmdb");
}

export function testTmdb(credential: string | null): Promise<void> {
  return invoke ? invoke("test_tmdb", { credential }) : post("/api/settings/tmdb/test", { credential });
}

export function saveTmdb(credential: string): Promise<TmdbStatus> {
  return invoke ? invoke("save_tmdb", { credential }) : post("/api/settings/tmdb", { credential });
}

interface LanguageSettingsView {
  languages: string[];
  translation_configured: boolean;
}

export function getLanguageSettings(): Promise<LanguageSettingsView> {
  return invoke ? invoke("language_settings") : request("/api/settings/languages", { cache: "no-store" });
}

export function saveLanguages(languages: string[]): Promise<LanguageSettingsView> {
  return invoke ? invoke("save_languages", { languages }) : post("/api/settings/languages", languages);
}

export function getOutputSettings(): Promise<OutputSettingsView> {
  return invoke ? invoke("output_settings") : request("/api/settings/output", { cache: "no-store" });
}

export function getEmbySettings(): Promise<EmbySettingsView> {
  return invoke ? invoke("emby_settings") : request("/api/settings/emby", { cache: "no-store" });
}

export function testEmby(input: EmbyInput): Promise<void> {
  return invoke ? invoke("test_emby", { input }) : post("/api/settings/emby/test", input);
}

export function saveEmby(input: EmbyInput): Promise<EmbySettingsView> {
  return invoke ? invoke("save_emby", { input }) : post("/api/settings/emby", input);
}

export function getEmbyCatalog(): Promise<EmbyLibraryPreview[] | null> {
  return invoke ? invoke("emby_catalog") : request("/api/emby/catalog", { cache: "no-store" });
}

export function importEmbyCatalog(): Promise<number> {
  return invoke ? invoke("import_emby_catalog") : post("/api/emby/import-all", null);
}

export function importEmbyLibrary(library: string): Promise<number> {
  return invoke ? invoke("import_emby", { library }) : post("/api/emby/import", library);
}

export function importEmbyItem(id: string, scope: EmbyScrapeScope): Promise<number> {
  return invoke ? invoke("import_emby_item", { id, scope }) :
    post(`/api/emby/items/${encodeURIComponent(id)}/scrape`, scope);
}

export function importEmbySelection(items: { id: string; scope: EmbyScrapeScope }[], useAi: boolean): Promise<number> {
  const input = { items, use_ai: useAi };
  return invoke ? invoke("import_emby_selection", { input }) : post("/api/emby/import-selection", input);
}

export function getAiStatus(): Promise<boolean> {
  return invoke ? invoke("ai_status") : request("/api/settings/ai", { cache: "no-store" });
}

export function createAiBatch(items: BatchInput[]): Promise<number> {
  const input = { items };
  return invoke ? invoke("create_ai_batch", { input }) : post("/api/batches/ai", input);
}

export function searchWorks(kind: MediaType, query: string): Promise<{ candidates: Candidate[]; notes: string[] }> {
  return invoke ? invoke("search_media", { kind, query }) : post("/api/search", { kind, query });
}

export function correctBatchMatch(batch: number, row: number, candidate: { provider: MediaProvider; id: string }): Promise<void> {
  return invoke ? invoke("correct_batch_match", { batch, row, candidate })
    : post(`/api/batches/${batch}/rows/${row}/match`, candidate);
}

export function overwriteEmby(target: EmbyScrapeTarget, replaceImages: boolean): Promise<void> {
  const input = { target, replace_images: replaceImages };
  return invoke ? invoke("overwrite_emby", { input }) : post("/api/emby/overwrite", input);
}

export function getEmbySeries(id: string): Promise<EmbySeriesPreview> {
  return invoke ? invoke("emby_series_preview", { id }) :
    request("/api/emby/series/" + encodeURIComponent(id), { cache: "no-store" });
}

export function getEmbyCredits(id: string): Promise<EmbyCredit[]> {
  return invoke ? invoke("emby_credits", { id }) : request(`/api/emby/credits/${encodeURIComponent(id)}`, { cache: "no-store" });
}

export type EmbyImage = "poster" | "backdrop" | "episode" | "portrait";

export async function loadEmbyImage(id: string, image: EmbyImage): Promise<string> {
  if (!invoke) throw new Error("Emby 图片暂不可用。");
  const bytes = await invoke<ArrayBuffer | number[]>("emby_preview_image", { id, image });
  return URL.createObjectURL(new Blob([Array.isArray(bytes) ? Uint8Array.from(bytes) : bytes]));
}

export function saveOutputSettings(options: OutputOptions): Promise<OutputSettingsView> {
  return invoke ? invoke("save_output_settings", { options }) : post("/api/settings/output", options);
}

export async function pickOutputDirectory(): Promise<string | null> {
  return invoke ? invoke("pick_output_directory") : null;
}

export function createBatch(items: BatchInput[]): Promise<number> {
  return invoke ? invoke("create_batch", { items }) : post("/api/batches", items);
}

export async function chooseMediaFolder(paths: string[] = []): Promise<FolderSelection | null> {
  if (invoke) return invoke("choose_media_folder");
  const preview = await post<FolderPreview>("/api/folder-preview", paths);
  return { folder: paths[0]?.split("/")[0] ?? "", paths, preview };
}

export function browseMountedFolders(path: string): Promise<string[]> {
  return request("/api/media-folders?" + new URLSearchParams({ path }), { cache: "no-store" });
}

export function previewMountedFolder(path: string): Promise<FolderSelection> {
  return post("/api/media-folders", path);
}

export function createFolderBatch(paths: string[]): Promise<FolderBatchResult> {
  return invoke ? invoke("create_folder_batch", { paths }) : post("/api/folder-batches", paths);
}

export function listBatches(before: number | null = null): Promise<BatchSummary[]> {
  return invoke ? invoke("list_batches", { before })
    : request("/api/batches" + (before === null ? "" : "?before=" + before));
}

export function getBatchView(batch: number): Promise<BatchView> {
  return invoke ? invoke("batch_view", { batch }) : request("/api/batches/" + batch + "/state");
}

export function controlBatch(batch: number, action: BatchAction): Promise<number> {
  return invoke ? invoke("control_batch", { batch, action }) : post("/api/batches/" + batch + "/control", action);
}

export function getBatchResult(batch: number, row: number): Promise<LookupResponse> {
  return invoke ? invoke("batch_result", { batch, row }) : request("/api/batches/" + batch + "/rows/" + row + "/result");
}

export function getBatchFiles(batch: number, row: number): Promise<string[]> {
  return invoke ? invoke("batch_files", { batch, row }) : request("/api/batches/" + batch + "/rows/" + row + "/files");
}

export function previewWriteback(batch: number, row: number | null): Promise<WritebackPreview> {
  return invoke ? invoke("preview_writeback", { batch, row }) : request(
    `/api/batches/${batch}/writeback` + (row === null ? "" : `?row=${row}`), { cache: "no-store" },
  );
}

export function queueWriteback(batch: number, row: number | null, token: string): Promise<number> {
  return invoke ? invoke("queue_writeback", { batch, row, token }) :
    post(`/api/batches/${batch}/writeback`, { row, token });
}

export function retryBatchRow(batch: number, row: number): Promise<void> {
  return invoke ? invoke("retry_batch_row", { batch, row }) : post("/api/batches/" + batch + "/rows/" + row + "/retry", {});
}

export function chooseBatchCandidate(batch: number, row: number, item: Candidate): Promise<void> {
  const candidate = { provider: item.provider, id: item.id };
  return invoke ? invoke("choose_batch_candidate", { batch, row, candidate })
    : post("/api/batches/" + batch + "/rows/" + row + "/candidate", candidate);
}
