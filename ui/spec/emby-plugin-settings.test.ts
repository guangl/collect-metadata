import { readFileSync } from "node:fs";
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { afterEach, expect, test, vi } from "vitest";

const html = readFileSync("emby-plugin/Web/settings.html", "utf8");
const script = readFileSync("emby-plugin/Web/settings.js", "utf8");
const pluginId = "0d06e6df-6aa6-4c32-94b3-ad9da31fa348";

afterEach(() => { document.body.innerHTML = ""; });

function mount(config: Record<string, unknown>) {
  const getPluginConfiguration = vi.fn(async () => config);
  const updatePluginConfiguration = vi.fn(async (_id: string, saved: Record<string, unknown>) => { config = saved; });
  const getVirtualFolders = vi.fn(async () => ({ Items: [
    { Name: "电影", CollectionType: "movies", Locations: ["/media/.private-video"] },
    { Name: "混合", CollectionType: "mixed", Locations: ["/media/mixed"] },
  ] }));
  document.body.innerHTML = html;
  const view = document.body.firstElementChild!;
  new Function("define", script)((_dependencies: string[], create: () => (this: unknown, view: Element) => void) => {
    create().call({ getApiClient: () => ({ getPluginConfiguration, updatePluginConfiguration, getVirtualFolders }) }, view);
  });
  return { ...within(document.body), updatePluginConfiguration };
}

test("configures a library and saves naming and sidecar options via the Emby plugin API", async () => {
  const page = mount({ OrganizeRoots: "", SyncToken: "saved-token" });
  const picker = await page.findByRole("option", { name: "电影 · /media/.private-video" });
  expect(page.queryByRole("option", { name: /混合/ })).toBeNull();
  fireEvent.change(page.getByRole("combobox", { name: "从媒体库添加路径" }), { target: { value: (picker as HTMLOptionElement).value } });
  fireEvent.click(page.getByRole("button", { name: "添加" }));
  const folder = page.getByLabelText<HTMLInputElement>("电影目录模板");
  const file = page.getByLabelText<HTMLInputElement>("电影文件名模板");
  fireEvent.input(folder, { target: { value: "../{title}" } });
  expect(folder.validity.valid).toBe(false);
  fireEvent.input(folder, { target: { value: "{title}{year_suffix}" } });
  expect(folder.validity.valid).toBe(true);
  fireEvent.input(file, { target: { value: "{title} - 正片" } });
  expect(page.getByText("电影示例 (2025)/电影示例 - 正片.mkv")).toBeTruthy();
  fireEvent.click(page.getByRole("checkbox", { name: "匹配的图片（同步重命名）" }));
  expect(page.getByLabelText<HTMLInputElement>("同步令牌").value).toBe("");
  fireEvent.submit(page.getByRole("button", { name: "保存设置" }).closest("form")!);
  await waitFor(() => expect(page.updatePluginConfiguration).toHaveBeenCalledWith(pluginId, expect.objectContaining({
    OrganizeRoots: "/media/.private-video", OrganizeEnabled: true, SyncToken: "saved-token",
    Organizer: expect.objectContaining({ MovieFolder: "{title}{year_suffix}", MovieFile: "{title} - 正片", MoveNfo: true, MoveImages: false }),
  })));
});

test("previews ani-rss presets, user samples, ranges and rename switches", async () => {
  const page = mount({});
  await waitFor(() => expect(page.getByRole<HTMLButtonElement>("button", { name: "保存设置" }).disabled).toBe(false));
  fireEvent.change(page.getByLabelText("命名预设"), { target: { value: "ani-group" } });
  fireEvent.click(page.getByRole("button", { name: "应用预设" }));
  expect(page.getByLabelText<HTMLInputElement>("分集文件名模板").value).toBe("[${subgroup}] ${title} S${seasonFormat}E${episodeFormat}");
  const episode = page.getByLabelText("分集样例");
  fireEvent.input(episode, { target: { value: "[字幕组] 动画 (2025) S01E07.mp4" } });
  expect(page.getByText("动画 (2025)/Season 1/[字幕组] 动画 S01E07.mp4")).toBeTruthy();
  fireEvent.input(episode, { target: { value: "动画 (2025) S01E07-E08.mp4" } });
  expect(page.getByText("动画 (2025)/Season 1/动画 (2025) S01E07-E08.mp4")).toBeTruthy();
  fireEvent.click(page.getByRole("checkbox", { name: "重命名分集文件" }));
  const field = page.getByLabelText<HTMLInputElement>("分集文件名模板");
  field.setSelectionRange(0, field.value.length);
  fireEvent.click(page.getByRole("button", { name: "{original}", exact: true }));
  expect(field.value).toBe("{original}");
  fireEvent.submit(page.getByRole("button", { name: "保存设置" }).closest("form")!);
  await waitFor(() => expect(page.updatePluginConfiguration).toHaveBeenCalledWith(pluginId, expect.objectContaining({
    Organizer: expect.objectContaining({ RenameEpisodes: false, EpisodeFile: "{original}", SeasonFolder: "Season ${season}" }),
  })));
});

test("shows environment-managed values without letting the form replace them", async () => {
  const page = mount({ OrganizeRoots: "/media/old", EnvironmentRoots: "/media/.private-video", SyncTokenOverridden: true });
  await page.findByText(/由容器的 EMBY_ORGANIZE_ROOTS/);
  const roots = page.getByRole<HTMLTextAreaElement>("textbox", { name: "允许整理的根路径" });
  expect(roots.value).toBe("/media/.private-video");
  expect(roots.readOnly).toBe(true);
  expect(page.getByLabelText<HTMLInputElement>("同步令牌").disabled).toBe(true);
  fireEvent.click(page.getByRole("switch", { name: "自动整理媒体库根目录" }));
  fireEvent.submit(page.getByRole("button", { name: "保存设置" }).closest("form")!);
  await waitFor(() => expect(page.updatePluginConfiguration).toHaveBeenCalledWith(pluginId, expect.objectContaining({
    OrganizeRoots: "/media/old", OrganizeEnabled: false,
  })));
});
