import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/main";
import { configureDatabase, getBatchView } from "../src/api";
import { parseBatch, terminal } from "../src/batch/engine";
import type { BatchRow } from "../src/batch/engine";
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
  raw: false, emby_nfo: false, poster: true, backdrop: false, stills: false, portraits: [] };

function response(data: unknown, status = 200) {
  return new Response(status === 204 ? null : JSON.stringify(data), { status });
}

function harness(initialRows: BatchRow[] = [], hasStorage = true) {
  const handle = vi.fn<(route: string, options?: RequestInit) => Promise<Response>>();
  let libraries = 0;
  let storage = storageOptions;
  let rows = initialRows;
  let status = "running";
  let id = initialRows.length > 0 ? 9 : 0;
  handle.mockImplementation(async (route, options) => {
    const body = options?.body ? JSON.parse(String(options.body)) : null;
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
    if (route === "/api/emby/libraries") {
      libraries += 1;
      return response(libraries === 1 ? [] : [{ id: "12", name: "测试库" }]);
    }
    if (route.startsWith("/api/media-folders?")) {
      const path = new URL(route, "http://fixture.test").searchParams.get("path");
      return response(path === "" ? ["NAS 电影"] : []);
    }
    if (route === "/api/media-folders") return response({ folder: `服务器共享目录 / ${body}`,
      paths: [`${body}/Movie.mkv`], preview: { works: [{ kind: "movie", query: "NAS 作品", file_count: 1 }],
        files: 1, skipped: 0, unrecognized: [] } });
    if (route === "/api/emby/preview") return response([{ id: "30", kind: "movie", title: "Emby 电影" }]);
    if (route === "/api/folder-preview") {
      return response({ works: [{ kind: "movie", query: "文件夹中的电影", file_count: 1 }], files: 1,
        skipped: 1, unrecognized: ["媒体/未知.mkv"] });
    }
    if (["/api/emby/import", "/api/folder-batches"].includes(route) || (route === "/api/batches" && body)) {
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
    if (route.endsWith("/files")) return response(Array.from({ length: 101 }, (_, n) => `媒体/${n}.mkv`));
    if (route.endsWith("/result")) return response({ record, saved, storage_error: null });
    if (route.endsWith("/candidate") || route.endsWith("/retry")) {
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
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("海报与详情", () => {
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
    const episodes = screen.getByText("分集列表", { exact: false }).closest("details")!;
    episodes.open = true;
    fireEvent(episodes, new Event("toggle"));
    await screen.findByText("第一集");
    setLabel("元数据语言", "ja");
    await screen.findByRole("heading", { name: "パラサイト" });
    expect(screen.getByText("剧情简介")).toBeTruthy();
    await screen.findByText("第1話");
    setLabel("元数据语言", "zh-CN");
    setLabel("按季查看", "0");
    await screen.findByRole("link", { name: "额外内容" });
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
    click(/寄生虫/);
    await screen.findByRole("heading", { name: title, exact: true });
    click("返回采集历史");
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

  it("在标签间切换并保存输出路径、命名和图片选项", async () => {
    const calls = harness();
    render(<App />);
    click("设置");
    await screen.findByLabelText("输出路径");
    setLabel("输出路径", "media");
    setLabel("作品目录名", "{title} ({year})");
    setLabel("资料文件名前缀", "{id}");
    fireEvent.click(screen.getByLabelText("演员", { exact: true }));
    fireEvent.click(screen.getByLabelText("背景图", { exact: true }));
    fireEvent.click(screen.getByLabelText("简介与宣传语", { exact: true }));
    expect(screen.getByText("寄生虫 (2019)-7/TMDB-movie-496243-metadata.json")).toBeTruthy();
    click("保存输出设置");
    await screen.findByText("已保存，设置应用于新建的文件夹批次。");
    const sent = calls.mock.calls.find(([route, options]) => route === "/api/settings/output" && options?.method === "POST");
    expect(JSON.parse(String(sent?.[1]?.body))).toMatchObject({
      directory: "media", directory_template: "{title} ({year})", filename_prefix: "{id}", portraits: ["actor"], backdrop: true, synopsis: false,
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
    click(/打开设置，PostgreSQL/);
    click("测试当前数据库");
    await screen.findByText("PostgreSQL 连接测试成功。");
    const tab = screen.getByRole("tab", { name: "PostgreSQL" });
    fireEvent.keyDown(tab, { key: "ArrowLeft" });
    expect(screen.getByRole("tabpanel", { name: "家庭 Emby" })).toBeTruthy();
    fireEvent.keyDown(tab, { key: "Home" });
    expect((screen.getByLabelText("输出路径") as HTMLInputElement).value).toBe("media");
    fireEvent.keyDown(tab, { key: "End" });
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
    click("打开 寄生虫 的详情");
    await screen.findByRole("heading", { name: title, level: 1 });
    await waitFor(() => expect(button("收起结果")).toBeTruthy());
    click("收起结果");
    click(/重试全部失败条目/);
    await screen.findByText(/已重新排入/);
    setLabel("候选作品", "0");
    click("采集所选作品");
    await waitFor(() => expect(screen.queryByRole("button", { name: "采集所选作品" })).toBeNull());
    fireEvent.click(within(screen.getByRole("listitem", { name: "失败条目" })).getByRole("button", { name: "重试", exact: true }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "重试", exact: true })).toBeNull());
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
    click("浏览服务器挂载目录");
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

  it("提交文件夹、手动列表和 Emby，并处理暂停、继续、取消", async () => {
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
    fireEvent.click(screen.getByRole("radio", { name: "Emby Server" }));
    expect(screen.queryByRole("button", { name: "选择媒体文件夹" })).toBeNull();
    click("读取媒体库");
    await screen.findByText("Emby 没有返回可导入的媒体库。");
    click("读取媒体库");
    await screen.findByLabelText("Emby 媒体库");
    setLabel("Emby 媒体库", "12");
    click("预览所选媒体库");
    const embyPosters = await screen.findByRole("list", { name: "待刮削作品" });
    expect(within(embyPosters).getByTitle("Emby 电影")).toBeTruthy();
    expect(calls.mock.calls.some(([route]) => route === "/api/emby/import")).toBe(false);
    click("开始刮削元数据");
    await screen.findByText(/已提交 Emby 媒体库/);
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
  calls.mockResolvedValueOnce(new Response("broken", { status: 503 }));
  await expect(getBatchView(100)).rejects.toThrow("503");
});
