import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/main";
import { Movie } from "../src/Movie";
import { configureDatabase, getBatchView } from "../src/api";
import { parseBatch, terminal } from "../src/batch/engine";
import type { BatchRow } from "../src/batch/engine";
import type { EmbyEpisodePreview, EmbyLibraryPreview } from "../src/types";
import { safeUrl, languageName } from "../src/Catalog";

const title = "寄生虫";
const cover = "https://image.tmdb.org/t/p/w342/fixture.jpg";
const candidate = { title, cover_url: cover, provider: "tmdb", id: "496243", year: 2019, original_languages: ["ko"] };
const saved = { id: 7, title, cover_url: cover, year: 2019, code: "TMDB-496243",
  media_type: "movie", collected_at: "2026-01-01T00:00:00Z", original_languages: ["ko"] };
const record = { schema_version: 2, media_type: "movie", query: title,
  fields: { title, cover_url: cover, description: "剧情简介", runtime_minutes: 132,
    original_languages: ["ko"], cast: ["演员"], rating: { value: 8, scale: 10, count: 20 },
    episodes: [{ id: 1, season: 1, number: 1, title: "第一集", date: "2024-01-01", url: null },
      { id: 2, season: 0, number: null, title: "额外内容", date: "2024-01-01", url: "https://www.themoviedb.org/" }] },
  field_sources: { runtime_minutes: "TMDB" },
  metadata_languages: ["zh-CN", "ja"],
  localized_metadata: { ja: { title: "パラサイト", "episodes/0/title": "第1話" } },
  sources: [{ provider: "TMDB", status: "ok", url: "https://www.themoviedb.org/movie/496243",
    fields: { cover_url: cover, director: "导演" }, data: { fixture: 1 } }] };
const storageOptions = { enabled: true, directory: "library", directory_template: "work-{row}", filename_prefix: "",
  metadata: true, synopsis: true, credits: true, ratings: true, episodes: true,
  raw: false, emby_nfo: false, poster: true, backdrop: false, stills: false, trailers: false, theme_videos: false, theme_music: false, extras: false, portraits: [] };

function response(data: unknown, status = 200) {
  return new Response(status === 204 ? null : JSON.stringify(data), { status });
}

function harness(initialRows: BatchRow[] = [], hasStorage = true, libraries: EmbyLibraryPreview[] | null = null, aiConfigured = false) {
  const handle = vi.fn<(route: string, options?: RequestInit) => Promise<Response>>();
  let storage = storageOptions;
  let rows = initialRows;
  let status = "running";
  let id = initialRows.length > 0 ? 9 : 0;
  handle.mockImplementation(async (route, options) => {
    const body = options?.body ? JSON.parse(String(options.body)) : null;
    if (route === "/api/settings/ai") return response(aiConfigured);
    if (route === "/api/search") return response({ candidates: [candidate], notes: [] });
    if (route === "/api/settings/postgres") return hasStorage ? response(null, 204) : response({ error: "未连接" }, 503);
    if (route === "/api/settings/output") {
      if (body) storage = body;
      return response({ root: "/output", options: storage });
    }
    if (route === "/api/settings/languages") return response({ languages: body ?? ["zh-CN"], translation_configured: false });
    if (route === "/api/settings/tmdb") return response(body ? "saved" : "environment");
    if (route === "/api/settings/tmdb/test") return response(null, 204);
    if (route === "/api/settings/emby") return response(body ? { enabled: body.enabled, server: body.server, configured: true } :
      { enabled: false, server: "", configured: false });
    if (route === "/api/settings/emby/test") return response(null, 204);
    if (route === "/api/history/7") return response({ record, saved, storage_error: null });
    if (route === "/api/history/languages") return response(["ko"]);
    if (route.startsWith("/api/history?")) return response(route.includes("before=") ? [] :
      route.includes("kind=series") ? [] : Array.from({ length: 50 }, (_, n) => n ? { ...saved, id: n + 9, title: "影片 " + n } : saved));
    if (route === "/api/emby/catalog") return response(libraries);
    if (/^\/api\/emby\/items\/\d+\/scrape$/.test(route) && options?.method === "POST") {
      id = 9;
      return response(id, 201);
    }
    if (route.startsWith("/api/media-folders?")) {
      const path = new URL(route, "http://fixture.test").searchParams.get("path");
      return response(path === "" ? ["NAS 电影"] : []);
    }
    if (route === "/api/media-folders") return response({ folder: `服务器共享目录 / ${body}`,
      paths: [`${body}/Movie.mkv`], preview: { works: [{ kind: "movie", query: "NAS 作品", file_count: 1 }],
        files: 1, skipped: 0, unrecognized: [] } });
    if (route === "/api/folder-preview") {
      return response({ works: [{ kind: "movie", query: "文件夹中的电影", file_count: 1 }], files: 1,
        skipped: 1, unrecognized: ["媒体/未知.mkv"] });
    }
    if (["/api/emby/import", "/api/emby/import-all", "/api/emby/import-selection", "/api/batches/ai", "/api/folder-batches"].includes(route) || (route === "/api/batches" && body)) {
      id = 9;
      status = "running";
      return response(route === "/api/folder-batches"
        ? { id, works: 1, files: 1, skipped: 1, unrecognized: ["媒体/未知.mkv"] } : id, 201);
    }
    if (route.startsWith("/api/batches?") || route === "/api/batches") {
      return response(id ? [{ id, created_at: saved.collected_at, folder_name: "媒体", status, total: rows.length, finished: 0, waiting: 0 }] : []);
    }
    if (route.endsWith("/state")) return response({ status, rows });
    if (route.endsWith("/control")) {
      if (body === "pause") status = "paused";
      if (body === "resume") status = "running";
      if (body === "cancel") status = "cancelled";
      return response(1);
    }
    if (route === "/api/batches/9/writeback" && body) {
      rows = rows.map((row) => ({
        ...row, writeback: "done", output: { written: 1, directory: "/media/film", failed: 0, message: null },
      }));
      return response(1);
    }
    if (route.startsWith("/api/batches/9/writeback")) {
      const done = rows[0]?.writeback === "done";
      return response({ token: "approved", writable: done ? 0 : 1, files: [{
        kind: "nfo", source: "/emby/film.mkv",
        path: done ? "/media/film.nfo" : null, backup: done ? "/media/.collect-metadata-backups/old.bak" : null,
        error: done ? null : "无法写回文件，请检查挂载和写入权限。",
        destinations: done ? [] : [{ path: "/media/film.nfo", action: "replace" }],
      }] });
    }
    if (route.endsWith("/files")) return response(Array.from({ length: 101 }, (_, n) => `媒体/${n}.mkv`));
    if (route.endsWith("/result")) return response({ record, saved, storage_error: null });
    if (route.endsWith("/candidate") || route.endsWith("/retry") || route.endsWith("/match")) {
      const rowId = Number(route.split("/")[5]);
      rows = rows.map((row) => row.id === rowId ? { ...row, stage: "success" } : row);
      return response(null, 204);
    }
    return response({ error: "找不到此资料。" }, 404);
  });
  vi.stubGlobal("fetch", handle);
  return handle;
}

function button(name: string | RegExp) {
  return screen.getByRole("button", { name, exact: typeof name === "string" });
}

function click(name: string | RegExp) {
  fireEvent.click(button(name));
}

function setLabel(name: string, value: string) {
  fireEvent.change(screen.getByLabelText(name, { exact: true }), { target: { value } });
}

function details(text: string) {
  const node = screen.getByText(text, { exact: true }).closest("details");
  if (!node) throw new Error("详情面板缺失");
  node.open = true;
  fireEvent(node, new Event("toggle"));
  return node;
}

beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  // jsdom doesn't implement the native dialog top layer.
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; this.dispatchEvent(new Event("close")); } },
  });
});
afterEach(() => {
  cleanup();
  Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal");
  Reflect.deleteProperty(HTMLDialogElement.prototype, "close");
  localStorage.clear();
  delete document.documentElement.dataset.theme;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("外观", () => {
  it("随系统变化，手动选择后跨加载保留，也可切回跟随系统", async () => {
    harness();
    const media = Object.assign(new EventTarget(), { matches: true });
    vi.stubGlobal("matchMedia", vi.fn(() => media));
    const { unmount } = render(<App />);
    await screen.findByText("暂无后台任务。提交刮削后，可以在这里查看进度。");
    expect(document.documentElement.dataset.theme).toBe("dark");
    act(() => { media.matches = false; media.dispatchEvent(new Event("change")); });
    expect(document.documentElement.dataset.theme).toBe("light");
    setLabel("外观主题", "dark");
    unmount();
    render(<App />);
    await screen.findByText("暂无后台任务。提交刮削后，可以在这里查看进度。");
    expect((screen.getByLabelText("外观主题") as HTMLSelectElement).value).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    setLabel("外观主题", "system");
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});

describe("海报与详情", () => {
  it("以浏览器时区显示 Emby 原始首播时间，并保留没有时区的日期", () => {
    const timestamp = "2015-10-04T16:00:00.0000000Z";
    const emby = {
      provider: "Emby", status: "ok" as const, note: null, url: "https://emby.test/",
      fields: { release_date: "2015-10-04" }, data: { item: { PremiereDate: timestamp } },
    };
    const snapshot = { record: {
      schema_version: 2, scope: "episode" as const, query: "最强的男人",
      fields: { title: "最强的男人", release_date: "2015-10-04" },
      field_sources: { release_date: "Emby" }, sources: [emby],
    }, saved: null, storage_error: null };
    const noop = () => {};
    try {
      vi.stubEnv("TZ", "Asia/Shanghai");
      const { rerender } = render(<Movie response={snapshot} onNotice={noop} onError={noop} />);
      expect(screen.getByText("2015-10-05").getAttribute("datetime")).toBe(timestamp);

      vi.stubEnv("TZ", "America/Los_Angeles");
      rerender(<Movie response={snapshot} onNotice={noop} onError={noop} />);
      expect(screen.getByText("2015-10-04").getAttribute("datetime")).toBe(timestamp);

      rerender(<Movie response={{ ...snapshot, record: {
        ...snapshot.record, fields: { ...snapshot.record.fields, release_date: "2015-10-05" },
        field_sources: { release_date: "TMDB" },
      } }} onNotice={noop} onError={noop} />);
      expect(screen.getByText("2015-10-05").getAttribute("datetime")).toBe("2015-10-05");
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("显示 Bangumi 正片、特别篇和角色配音", () => {
    const snapshot = { record: {
      schema_version: 2, media_type: "series" as const, query: "动画",
      fields: { title: "动画", bangumi_id: "253", episodes: [
        { id: 1, season: null, number: 1, title: "正片第一集", date: null, url: null, runtime_minutes: null },
        { id: 2, season: 0, number: null, title: "额外故事", date: null, url: null, runtime_minutes: null },
      ] },
      field_sources: { episodes: "Bangumi" },
      sources: [{
        provider: "Bangumi", status: "ok" as const, note: null, url: "https://bgm.tv/subject/253", fields: {},
        data: {
          subject: { type: 2 },
          characters: [{ name: "主角", actors: [{ name: "配音演员", images: { medium: "https://lain.bgm.tv/pic/crt/actor.jpg" } }] }],
          persons: [{ type: 1, name: "编剧甲", relation: "脚本" }],
        },
      }],
    }, saved: null, storage_error: null };
    const noop = () => {};
    render(<Movie response={snapshot} onNotice={noop} onError={noop} />);

    fireEvent.click(screen.getByRole("tab", { name: "季与分集" }));
    const episodes = screen.getByText("分集列表").closest("details")!;
    episodes.open = true;
    fireEvent(episodes, new Event("toggle"));
    expect(screen.getByText("正片第一集")).toBeTruthy();
    expect(screen.queryByText("额外故事")).toBeNull();
    setLabel("分集类别", "0");
    expect(screen.getByText("额外故事")).toBeTruthy();
    expect(screen.queryByText("正片第一集")).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "演职员" }));
    const voice = within(screen.getByRole("region", { name: "声优" }));
    expect(voice.getByText("主角")).toBeTruthy();
    expect(voice.getByText("配音演员").closest("li")?.querySelector("img")?.src)
      .toBe("https://lain.bgm.tv/pic/crt/actor.jpg");
    expect(within(screen.getByRole("region", { name: "编剧" })).getByText("编剧甲")).toBeTruthy();
  });

  it("默认打开刮削工作台，导航不提供独立作品采集页", async () => {
    harness();
    render(<App />);
    expect(screen.getByRole("heading", { name: "刮削工作台", level: 1 })).toBeTruthy();
    const nav = within(screen.getByRole("navigation", { name: "工作区" }));
    expect(nav.getAllByRole("button").map((item) => item.textContent)).toEqual([
      "刮削工作台", "采集历史", "设置", "关于",
    ]);
    expect(nav.getByRole("button", { name: "刮削工作台" }).getAttribute("aria-current")).toBe("page");
    await screen.findByText("暂无后台任务。提交刮削后，可以在这里查看进度。");
    click("采集历史");
    expect(nav.getByRole("button", { name: "采集历史" }).getAttribute("aria-current")).toBe("page");
    click("Collect Metadata");
    expect(screen.getByRole("heading", { name: "刮削工作台", level: 1 })).toBeTruthy();
  });

  it("从历史海报打开详情、切换元数据语言并导出资料", async () => {
    harness();
    render(<App />);
    click("采集历史");
    const poster = await screen.findByRole("button", { name: /寄生虫/ });
    const image = poster.querySelector("img")!;
    expect(image.getAttribute("src")).toBe(cover);
    fireEvent.error(image);
    expect(poster.querySelector("img")).toBeNull();
    fireEvent.click(poster);
    await screen.findByRole("heading", { name: title, exact: true });
    expect(screen.getByText("剧情简介")).toBeTruthy();
    expect(screen.getByText("132 分钟")).toBeTruthy();
    expect(screen.getByText("8 / 10 · 20 人评价")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "季与分集" }));
    const episodes = screen.getByText("分集列表", { exact: false }).closest("details")!;
    episodes.open = true;
    fireEvent(episodes, new Event("toggle"));
    await screen.findByText("第一集");
    setLabel("元数据语言", "ja");
    await screen.findByRole("heading", { name: "パラサイト" });
    await screen.findByText("第1話");
    fireEvent.click(screen.getByRole("tab", { name: "简介与资料" }));
    expect(screen.getByText("剧情简介")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "季与分集" }));
    setLabel("元数据语言", "zh-CN");
    setLabel("按季查看", "0");
    await screen.findByRole("link", { name: "额外内容" });
    fireEvent.click(screen.getByRole("tab", { name: "数据来源" }));
    const source = screen.getByRole("heading", { name: "TMDB", exact: true }).closest("details")!;
    source.open = true;
    details("查看来源 JSON");
    await screen.findByText(/"fixture": 1/);
    Object.defineProperty(URL, "createObjectURL", { value: vi.fn(() => "blob:fixture"), configurable: true });
    Object.defineProperty(URL, "revokeObjectURL", { value: vi.fn(), configurable: true });
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    click("导出 JSON");
    await screen.findByText("已导出 JSON。");
    click("返回采集历史");
    expect(screen.getByRole("heading", { name: "采集历史" })).toBeTruthy();
  });

  it("从历史海报进入详情后回到原列表和筛选条件", async () => {
    const calls = harness();
    render(<App />);
    click("采集历史");
    await screen.findByRole("button", { name: /寄生虫/ });
    setLabel("原始语言", "ko");
    await waitFor(() => expect(calls).toHaveBeenCalledWith("/api/history?language=ko", undefined));
    const poster = button(/寄生虫/);
    fireEvent.click(poster);
    const dialog = await screen.findByRole("dialog", { name: "作品详情" });
    const heading = within(dialog).getByRole("heading", { name: title, exact: true });
    expect(document.activeElement).toBe(button("返回采集历史"));
    fireEvent.pointerDown(heading);
    fireEvent.click(heading);
    expect(dialog.isConnected).toBe(true);
    fireEvent.pointerDown(heading);
    fireEvent.click(dialog);
    expect(dialog.isConnected).toBe(true);
    fireEvent.pointerDown(dialog);
    fireEvent.click(dialog);
    expect(dialog.isConnected).toBe(false);
    expect(document.activeElement).toBe(poster);
    expect((screen.getByLabelText("原始语言") as HTMLSelectElement).value).toBe("ko");
    click("加载更早的记录");
    await waitFor(() => expect(screen.queryByRole("button", { name: "加载更早的记录" })).toBeNull());
    click("刷新");
    setLabel("作品类型", "series");
    await screen.findByText("没有符合条件的采集记录。");
    click("关于");
    expect(screen.getByRole("heading", { name: "关于 Collect Metadata" })).toBeTruthy();
  });
});

describe("设置", () => {
  it("多选元数据语言并保存首选顺序", async () => {
    const calls = harness();
    render(<App />);
    click("设置");
    fireEvent.click(screen.getByRole("tab", { name: "元数据语言" }));
    const chinese = await screen.findByRole("checkbox", { name: "简体中文（首选）" });
    expect((chinese as HTMLInputElement).checked).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: "日语" }));
    fireEvent.change(screen.getByRole("combobox", { name: "首选语言" }), { target: { value: "ja" } });
    click("保存语言");
    await screen.findByText("已保存，将用于后续采集和同步。");
    const sent = calls.mock.calls.find(([route, options]) => route === "/api/settings/languages" && options?.method === "POST");
    expect(JSON.parse(String(sent?.[1]?.body))).toEqual(["ja", "zh-CN"]);
  });

  it("在标签间切换并保存输出路径、命名和资料选项", async () => {
    const calls = harness();
    render(<App />);
    click("设置");
    await screen.findByLabelText("输出路径");
    setLabel("输出路径", "media");
    setLabel("作品目录名", "{title} ({year})");
    setLabel("资料文件名前缀", "{id}");
    fireEvent.click(screen.getByLabelText("演员", { exact: true }));
    fireEvent.click(screen.getByLabelText("背景图", { exact: true }));
    fireEvent.click(screen.getByLabelText("预告片与先导预告"));
    fireEvent.click(screen.getByLabelText("主题视频", { exact: true }));
    fireEvent.click(screen.getByLabelText("简介与宣传语", { exact: true }));
    expect(screen.getByText("寄生虫 (2019)-7/TMDB-movie-496243-metadata.json")).toBeTruthy();
    click("保存输出设置");
    await screen.findByText("已保存，设置应用于新的刮削任务。");
    const sent = calls.mock.calls.find(([route, options]) => route === "/api/settings/output" && options?.method === "POST");
    expect(JSON.parse(String(sent?.[1]?.body))).toMatchObject({
      directory: "media", directory_template: "{title} ({year})", filename_prefix: "{id}", portraits: ["actor"], backdrop: true, synopsis: false, trailers: true, theme_videos: true,
    });
    fireEvent.keyDown(screen.getByRole("tab", { name: "输出与命名" }), { key: "ArrowRight" });
    expect(screen.getByRole("tabpanel", { name: "TMDB" })).toBeTruthy();
    click("测试连接");
    await screen.findByText("TMDB 连接测试成功。");
    setLabel("TMDB API 凭据", "fixture");
    click("保存配置");
    await screen.findByText("已保存 TMDB 凭据");
    fireEvent.click(screen.getByRole("tab", { name: "家庭 Emby" }));
    setLabel("Emby 服务地址", "https://emby.example.test");
    setLabel("Emby API 密钥", "fixture");
    fireEvent.click(screen.getByLabelText("优先读取 Emby 媒体库"));
    click("测试 Emby 连接");
    await screen.findByText("Emby 媒体库读取成功。");
    click("保存 Emby 设置");
    await screen.findByText("已保存 Emby 设置。");
    expect(screen.getByRole("link", { name: "打开 Emby 自动整理设置" }).getAttribute("href"))
      .toBe("https://emby.example.test/web/index.html#!/configurationpage?name=collectmetadata");
    click(/打开设置，PostgreSQL/);
    click("测试当前数据库");
    await screen.findByText("PostgreSQL 连接测试成功。");
    const tab = screen.getByRole("tab", { name: "PostgreSQL" });
    fireEvent.keyDown(tab, { key: "ArrowLeft" });
    expect(screen.getByRole("tabpanel", { name: "家庭 Emby" })).toBeTruthy();
    fireEvent.keyDown(tab, { key: "Home" });
    expect((screen.getByLabelText("输出路径") as HTMLInputElement).value).toBe("media");
    fireEvent.keyDown(tab, { key: "End" });
    const diagnostics = screen.getByRole("tabpanel", { name: "日志与排查" });
    expect(within(diagnostics).getByText("docker compose logs --since 30m")).toBeTruthy();
    expect(within(diagnostics).getByRole("button", { name: "下载排查手册" })).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "元数据语言" }));
    expect(screen.getByRole("tabpanel", { name: "元数据语言" })).toBeTruthy();
    await screen.findByRole("checkbox", { name: "简体中文（首选）" });
  });
});

describe("后台批次", () => {
  const item: BatchRow = { id: 1, kind: "movie", query: title, stage: "success", message: null,
    candidates: null, has_result: true, title, original_languages: ["ko"], file_count: 101, cover_url: cover,
    output: { written: 2, directory: "/output/film", failed: 0, message: null } };
  it("在批次中查看海报、候选、关联路径并执行控制操作", async () => {
    harness([item, { ...item, id: 2, query: "选择条目", stage:"choice", candidates: [candidate], has_result: false, file_count: 0, output: null },
      { ...item, id: 3, query: "失败条目", stage: "failed", has_result: false, file_count: 0, output: null }]);
    render(<App />);
    await screen.findByText("/output/film");
    details("101 个关联文件");
    await screen.findByText("媒体/0.mkv");
    click("显示更多文件");
    expect(screen.getByText("媒体/100.mkv")).toBeTruthy();
    const poster = button("打开 寄生虫 的详情");
    fireEvent.click(poster);
    const dialog = await screen.findByRole("dialog", { name: "作品详情" });
    expect(within(dialog).getByRole("heading", { name: title, level: 1 })).toBeTruthy();
    expect(document.activeElement).toBe(button("收起结果"));
    fireEvent(dialog, new Event("cancel", { cancelable: true }));
    expect(dialog.isConnected).toBe(false);
    expect(document.activeElement).toBe(poster);
    click(/重试全部失败条目/);
    await screen.findByText(/已重新排入/);
    setLabel("候选作品", "0");
    click("采集所选作品");
    await waitFor(() => expect(screen.queryByRole("button", { name: "采集所选作品" })).toBeNull());
    fireEvent.click(within(screen.getByRole("listitem", { name: "失败条目" })).getByRole("button", { name: "重试", exact: true }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "重试", exact: true })).toBeNull());
  });

  it("预览部分写回中的失败文件，确认后能查看原件备份", async () => {
    const calls = harness([{ ...item, writeback: "failed" }]);
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "查看失败并重试" }));
    const preview = await screen.findByRole("dialog", { name: "Emby 媒体写回" });
    await within(preview).findByText("替换并备份 ·", { exact: false });
    expect(calls.mock.calls.some(([route, options]) => route.endsWith("/writeback") && options?.method === "POST")).toBe(false);
    fireEvent.click(within(preview).getByRole("button", { name: "确认写回（1 项）" }));
    await screen.findByText("已排入 1 条写回任务。");
    const confirmation = calls.mock.calls.find(([route, options]) => route.endsWith("/writeback") && options?.method === "POST");
    expect(JSON.parse(String(confirmation?.[1]?.body))).toEqual({ row: 1, token: "approved" });
    fireEvent.click(await screen.findByRole("button", { name: "查看写回记录" }));
    await screen.findByText("/media/.collect-metadata-backups/old.bak");
    click("关闭");
    expect(screen.queryByRole("dialog", { name: "Emby 媒体写回" })).toBeNull();
  });

  it("不连接 PostgreSQL 也能预览海报，但不能提交刮削", async () => {
    const calls = harness([], false);
    const { container } = render(<App />);
    await screen.findByText("连接 PostgreSQL 后即可提交后台批次。");
    expect((button("选择媒体文件夹") as HTMLButtonElement).disabled).toBe(false);
    const file = new File([""], "movie.mkv");
    Object.defineProperty(file, "webkitRelativePath", { value: "电影/movie.mkv" });
    fireEvent.change(container.querySelector("input[type=file]")!, { target: { files: [file] } });
    await screen.findByRole("list", { name: "待刮削作品" });
    expect((button("开始刮削元数据") as HTMLButtonElement).disabled).toBe(true);
    expect(calls.mock.calls.some(([route]) => route === "/api/folder-batches")).toBe(false);
  });

  it("浏览服务器挂载目录，预览后才提交共享文件", async () => {
    const calls = harness();
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "浏览服务器挂载目录" }));
    await screen.findByLabelText("子目录");
    setLabel("子目录", "NAS 电影");
    click("打开子目录");
    await screen.findByText("共享目录 / NAS 电影");
    click("上一级");
    await screen.findByLabelText("子目录");
    setLabel("子目录", "NAS 电影");
    click("打开子目录");
    await screen.findByText("共享目录 / NAS 电影");
    click("预览当前挂载目录");
    const posters = await screen.findByRole("list", { name: "待刮削作品" });
    expect(within(posters).getByTitle("NAS 作品")).toBeTruthy();
    expect(calls.mock.calls.some(([route]) => route === "/api/folder-batches")).toBe(false);
    click("开始刮削元数据");
    await screen.findByText(/已提交 1 部作品/);
    const submission = calls.mock.calls.find(([route]) => route === "/api/folder-batches");
    expect(JSON.parse(String(submission?.[1]?.body))).toEqual(["NAS 电影/Movie.mkv"]);
  });

  it("提交文件夹和手动列表，并处理暂停、继续、取消", async () => {
    const calls = harness([{ ...item, stage: "queued", has_result: false, file_count: 0, output: null }]);
    const { container } = render(<App />);
    await screen.findByRole("button", { name: "暂停批次" });
    click("暂停批次");
    await screen.findByRole("button", { name: "继续批次" });
    click("继续批次");
    await screen.findByRole("button", { name: "取消批次" });
    click("取消批次");
    click("返回");
    click("取消批次");
    click("确认取消");
    await screen.findByText("已取消", { exact: true });
    const files = container.querySelector<HTMLInputElement>("input[type=file]")!;
    const file = new File([""], "work.mkv");
    Object.defineProperty(file, "webkitRelativePath", { value: "媒体/work.mkv" });
    fireEvent.change(files, { target: { files: [file] } });
    const posters = await screen.findByRole("list", { name: "待刮削作品" });
    expect(within(posters).getByTitle("文件夹中的电影")).toBeTruthy();
    expect(screen.getByText("识别到 1 部作品，关联 1 个文件，跳过 1 个文件。")).toBeTruthy();
    expect(calls.mock.calls.some(([route]) => route === "/api/folder-batches")).toBe(false);
    details("查看未识别的视频");
    await screen.findByText("媒体/未知.mkv");
    click("取消预览");
    expect(screen.queryByRole("list", { name: "待刮削作品" })).toBeNull();
    expect(calls.mock.calls.some(([route]) => route === "/api/folder-batches")).toBe(false);
    fireEvent.change(files, { target: { files: [file] } });
    await screen.findByRole("list", { name: "待刮削作品" });
    click("开始刮削元数据");
    await screen.findByText(/已提交 1 部作品/);
    const submitted = calls.mock.calls.filter(([route]) => route === "/api/folder-batches");
    expect(submitted).toHaveLength(1);
    expect(JSON.parse(String(submitted[0][1]?.body))).toEqual(["媒体/work.mkv"]);
    await screen.findByRole("list", { name: "批次作品海报" });
    details("手动输入作品");
    setLabel("作品清单", "电影：片名\n剧集：剧名\nABC-123");
    setLabel("默认类型", "av");
    click("提交后台采集");
    await waitFor(() => expect(calls.mock.calls.some(([route, options]) => route === "/api/batches" && options?.method === "POST")).toBe(true));
  });

  it("切换 Emby 媒体库和详情标签，读取演职员头像并锁定背景滚动",  async () => {
    const film = { id: "30", kind: "movie" as const, title: "Emby 电影", year: 2019,
      original_title: "Poster Movie", overview: "Emby 已有剧情简介", genres: ["悬疑"], rating: 8.5,
      imdb_rating: { value: 8.6, count: 12345 }, critic_rating: 96, official_rating: "R",
      bangumi_id: "303162", tags: ["剧情"],
      runtime_minutes: 120, poster: "fixture", backdrop: "wide" };
    const calls = harness([], true, [
      { id: "12", name: "电影馆", works: [film] },
      { id: "13", name: "剧集馆", works: [{ ...film, id: "31", title: "Emby 剧集", kind: "series", poster: null }] },
      { id: "14", name: "空媒体库", works: [] },
    ]);
    const episode: EmbyEpisodePreview = { id: "34", number: 1, last_number: null, title: "第一集",
      original_title: "First Episode", overview: "这是第一集的简介。", date: "2017-12-01",
      runtime_minutes: 43, rating: 9.2, image: "still" };
    const defaultRequest = calls.getMockImplementation()!;
    calls.mockImplementation((route, options) => route === "/api/emby/series/31"
      ? Promise.resolve(response({ partial: false, seasons: [
        { number: 1, title: "第一季", episodes: [episode] },
        { number: 2, title: "第二季", episodes: [{ ...episode, id: "35", title: "回归", overview: "这是第二季第一集的简介。" }] },
      ] }))
      : route === "/api/emby/credits/30"
        ? Promise.resolve(response([
          { id: "99", name: "测试导演", role: "director", detail: null, portrait: "director" },
          { id: "98", name: "测试声优", role: "voice", detail: "主角", portrait: "voice" },
        ]))
        : defaultRequest(route, options));
    const { container } = render(<App />);
    const filmButton = await screen.findByRole("button", { name: /^Emby 电影/ });
    const filmPanel = screen.getByRole("tabpanel", { name: "电影馆" });
    expect(filmPanel.hasAttribute("tabindex")).toBe(false);
    expect(within(filmPanel).getByRole("button", { name: /^Emby 电影/ })).toBe(filmButton);
    expect(filmButton.tabIndex).toBe(0);
    expect(screen.getByRole("radio", { name: "Emby Server" })).toHaveProperty("checked", true);
    expect(screen.getByRole("tab", { name: "剧集馆" }).getAttribute("aria-selected")).toBe("false");
    expect(screen.getByRole("tab", { name: "空媒体库" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "剧集馆" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Emby 剧集/ })).toBeNull();
    expect(screen.getByText("3 个媒体库 · 2 部作品")).toBeTruthy();
    expect(filmButton.querySelector("img")?.getAttribute("src")).toBe("/api/emby/images/30/poster?v=fixture");
    expect(calls.mock.calls.some(([route]) => route.startsWith("/api/emby/credits/"))).toBe(false);
    expect(calls.mock.calls.some(([route]) => route === "/api/emby/import-all")).toBe(false);

    fireEvent.click(filmButton);
    const detail = screen.getByRole("dialog", { name: "Emby 电影" });
    expect(within(detail).getByText("Emby 已有剧情简介")).toBeTruthy();
    expect(within(detail).getByLabelText("评分 8.5，满分 10")).toBeTruthy();
    expect(within(detail).getByText("8.6 / 10 · 12,345 人评价")).toBeTruthy();
    expect(within(detail).getByText("96 / 100")).toBeTruthy();
    expect(within(detail).getByText("303162")).toBeTruthy();
    expect(within(detail).getByText("剧情")).toBeTruthy();
    expect(detail.querySelector(".work-backdrop img")?.getAttribute("src")).toBe("/api/emby/images/30/backdrop?v=wide");
    expect(document.documentElement.style.overflow).toBe("hidden");
    fireEvent.click(within(detail).getByRole("tab", { name: "演职员" }));
    const director = await within(detail).findByText("测试导演");
    expect(director.closest("li")?.querySelector("img")?.getAttribute("src")).toBe("/api/emby/images/99/portrait?v=director");
    expect(within(detail).getByRole("heading", { name: "声优" })).toBeTruthy();
    expect(within(detail).getByText("测试声优")).toBeTruthy();
    expect(within(detail).queryByText("Emby 已有剧情简介")).toBeNull();
    click("关闭详情");
    expect(container.querySelector("dialog")).toBeNull();
    expect(document.documentElement.style.overflow).toBe("");
    expect(calls.mock.calls.some(([route]) => route.startsWith("/api/emby/series/"))).toBe(false);

    fireEvent.keyDown(screen.getByRole("tab", { name: "电影馆" }), { key: "ArrowRight" });
    expect(screen.queryByRole("button", { name: /^Emby 电影/ })).toBeNull();
    expect(button(/^Emby 剧集/).querySelector("img")).toBeNull();
    click(/^Emby 剧集/);
    click("刮削此剧集");
    await screen.findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.find(([route]) => route === "/api/emby/items/31/scrape")?.[1]?.body).toBe('"series"');
    fireEvent.click(screen.getByRole("tab", { name: "季与分集" }));
    await screen.findByRole("heading", { name: "季与分集" });
    fireEvent.click(await screen.findByRole("button", { name: "第 2 季" }));
    const episodeButton = button(/回归/);
    fireEvent.click(episodeButton);
    const episodeDetail = screen.getByRole("dialog", { name: "回归" });
    expect(within(episodeDetail).getByText("这是第二季第一集的简介。")).toBeTruthy();
    expect(within(episodeDetail).getByText("2017-12-01")).toBeTruthy();
    expect(episodeDetail.querySelector(".episode-detail img")?.getAttribute("src")).toBe("/api/emby/images/35/episode?v=still");
    fireEvent.click(within(episodeDetail).getByRole("button", { name: "刮削此分集" }));
    await within(episodeDetail.querySelector(".episode-detail")!).findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.find(([route]) => route === "/api/emby/items/35/scrape")?.[1]?.body).toBe('"episode"');
    click(/返回分集/);
    expect(button("第 2 季").getAttribute("aria-pressed")).toBe("true");
    expect(document.activeElement).toBe(episodeButton);
    click("关闭详情");
    click("刮削全部媒体库");
    await screen.findByText(/已将 Emby 的全部媒体库提交/);
    expect(calls.mock.calls.filter(([route]) => route === "/api/emby/import-all")).toHaveLength(1);
    expect(screen.getByRole("tab", { name: "电影馆" })).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "空媒体库" }));
    expect(screen.getByRole("tabpanel", { name: "空媒体库" }).tabIndex).toBe(0);
  });

  it("在 Emby 库中搜索、筛缺资料并保持跨筛选选择，支持普通和 AI 刮削", async () => {
    const film = { id: "30", kind: "movie" as const, title: "红辣椒", year: 2006,
      original_title: "Paprika", overview: "梦境", genres: [], rating: null,
      runtime_minutes: null, poster: null, backdrop: null };
    const series = { ...film, id: "31", kind: "series" as const, title: "暗黑",
      original_title: "Dark", overview: null, poster: "art", year: 2017 };
    const calls = harness([], true, [{ id: "7", name: "影片库", works: [film, series] }], true);
    render(<App />);
    await screen.findByRole("checkbox", { name: "选择 红辣椒" });
    setLabel("资料完整性", "poster");
    expect(screen.queryByRole("checkbox", { name: "选择 暗黑" })).toBeNull();
    click("选择当前结果");
    expect(screen.getByRole("checkbox", { name: "选择 红辣椒" })).toHaveProperty("checked", true);
    setLabel("资料完整性", "overview");
    setLabel("搜索片名", "Ｄａｒｋ");
    fireEvent.click(screen.getByRole("checkbox", { name: "选择 暗黑" }));
    expect(screen.getByText("显示 1 部 · 已选 2 部")).toBeTruthy();
    click("AI 刮削所选");
    await screen.findByText("已将所选 2 部作品提交到 AI 刮削。");
    const requests = () => calls.mock.calls.filter(([route]) => route === "/api/emby/import-selection");
    expect(JSON.parse(String(requests()[0][1]?.body))).toEqual({
      items: [{ id: "30", scope: "movie" }, { id: "31", scope: "series" }], use_ai: true,
    });
    await waitFor(() => expect(button("刮削所选")).toHaveProperty("disabled", true));
    fireEvent.click(screen.getByRole("checkbox", { name: "选择 暗黑" }));
    click("刮削所选");
    await waitFor(() => expect(requests()).toHaveLength(2));
    expect(JSON.parse(String(requests()[1][1]?.body))).toEqual({
      items: [{ id: "31", scope: "series" }], use_ai: false,
    });
  });

  it("在原批次条目搜索新匹配或直接按 TMDB ID 重新采集", async () => {
    const row: BatchRow = { id: 19, query: "原条目", title: null, kind: "movie", stage: "not_found",
      message: null, has_result: false, candidates: null, cover_url: null,
      original_languages: [], file_count: 1, output: null };
    const calls = harness([row]);
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "纠正匹配" }));
    setLabel("片名或 TMDB ID", "寄生虫");
    click("搜索作品");
    await screen.findByRole("option", { name: /寄生虫 · 2019/ });
    setLabel("搜索结果", "0");
    click("重新采集并记住");
    await screen.findByText(/已提交纠正匹配/);
    const requests = () => calls.mock.calls.filter(([route]) => route === "/api/batches/9/rows/19/match");
    expect(JSON.parse(String(requests()[0][1]?.body))).toEqual({ provider: "tmdb", id: "496243" });
    click("纠正匹配");
    setLabel("片名或 TMDB ID", "tmdb:1234");
    click("重新采集并记住");
    await waitFor(() => expect(requests()).toHaveLength(2));
    expect(JSON.parse(String(requests()[1][1]?.body))).toEqual({ provider: "tmdb", id: "1234" });
  });

  it("按当前库或单部影片提交刮削，并在确认后单独向 Emby 提交覆盖", async () => {
    const film = { id: "30", kind: "movie" as const, title: "红辣椒", year: 2006,
      original_title: null, overview: null, genres: [], rating: null,
      runtime_minutes: null, poster: null, backdrop: null };
    const calls = harness([], true, [
      { id: "7", name: "动画电影", works: [film] },
      { id: "8", name: "私人影片", works: [{ ...film, id: "40", title: "家庭旅行", year: null }] },
    ]);
    const defaultRequest = calls.getMockImplementation()!;
    const overwriteRequests = () => calls.mock.calls.filter(([route]) => route === "/api/emby/overwrite");
    calls.mockImplementation((route, options) => route === "/api/emby/overwrite"
      ? Promise.resolve(response(null, 204))
      : defaultRequest(route, options));
    render(<App />);
    const animated = await screen.findByRole("tabpanel", { name: "动画电影" });
    fireEvent.click(within(animated).getByRole("button", { name: "刮削此媒体库" }));
    await within(animated).findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.find(([route]) => route === "/api/emby/import")?.[1]?.body).toBe('"7"');

    click(/^红辣椒/);
    const animatedFilm = screen.getByRole("dialog", { name: "红辣椒" });
    fireEvent.click(within(animatedFilm).getByRole("button", { name: "刮削此影片" }));
    await within(animatedFilm).findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.find(([route]) => route === "/api/emby/items/30/scrape")?.[1]?.body).toBe('"movie"');
    click("关闭详情");

    fireEvent.click(screen.getByRole("tab", { name: "私人影片" }));
    const privateLibrary = screen.getByRole("tabpanel", { name: "私人影片" });
    fireEvent.click(within(privateLibrary).getByRole("button", { name: "刮削此媒体库" }));
    await within(privateLibrary).findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.filter(([route]) => route === "/api/emby/import").map(([, options]) => options?.body))
      .toEqual(['"7"', '"8"']);
    fireEvent.click(within(privateLibrary).getByRole("button", { name: "覆盖刮削" }));
    expect(within(privateLibrary).getByText(/「私人影片」/)).toBeTruthy();
    expect(overwriteRequests()).toHaveLength(0);
    fireEvent.click(within(privateLibrary).getByRole("button", { name: "确认覆盖刮削" }));
    await within(privateLibrary).findByText("已将覆盖刮削请求提交给 Emby。");
    expect(JSON.parse(String(overwriteRequests()[0][1]?.body))).toEqual({
      target: { type: "library", id: "8" }, replace_images: false,
    });

    click(/^家庭旅行/);
    const video = screen.getByRole("dialog", { name: "家庭旅行" });
    fireEvent.click(within(video).getByRole("button", { name: "刮削此影片" }));
    await within(video).findByText("已提交后台任务 #9。");
    expect(calls.mock.calls.find(([route]) => route === "/api/emby/items/40/scrape")?.[1]?.body).toBe('"movie"');
    const overwrite = () => fireEvent.click(within(video).getByRole("button", { name: "覆盖刮削" }));
    overwrite();
    fireEvent.click(within(video).getByRole("button", { name: "取消" }));
    expect(overwriteRequests()).toHaveLength(1);
    overwrite();
    fireEvent.click(within(video).getByRole("checkbox", { name: "同时替换已有图片" }));
    fireEvent.click(within(video).getByRole("button", { name: "确认覆盖刮削" }));
    await within(video).findByText("已将覆盖刮削请求提交给 Emby。");
    expect(overwriteRequests()).toHaveLength(2);
    expect(JSON.parse(String(overwriteRequests()[1][1]?.body))).toEqual({
      target: { type: "item", id: "40", scope: "movie" }, replace_images: true,
    });
    expect(calls.mock.calls.some(([route]) => route === "/api/emby/import-all")).toBe(false);
  });
});

it("识别非法输入及旧版本接口失败", async () => {
  const calls = harness();
  expect(() => parseBatch("  ", "movie")).toThrow("至少输入");
  expect(() => parseBatch(Array(101).fill("item").join("\n"), "movie")).toThrow("最多");
  expect(terminal("cancelled")).toBe(true);
  expect(safeUrl("https://a:b@example.com/p.jpg")).toBeNull();
  expect(safeUrl("broken")).toBeNull();
  expect(languageName("1-invalid")).toBe("1-invalid");
  await expect(configureDatabase("none")).rejects.toThrow("服务端");
  const id = "1234567890abcdef1234567890abcdef";
  calls.mockResolvedValueOnce(new Response(JSON.stringify({ error: "数据库不可用" }), {
    status: 503, headers: { "x-request-id": id },
  }));
  await expect(getBatchView(100)).rejects.toThrow("数据库不可用（诊断编号 " + id + "）");
  calls.mockRejectedValueOnce(new TypeError("failed to fetch"));
  await expect(getBatchView(100)).rejects.toThrow("无法连接 Collect Metadata Web");
  calls.mockResolvedValueOnce(new Response("broken", { status: 503 }));
  await expect(getBatchView(100)).rejects.toThrow("503");
});
