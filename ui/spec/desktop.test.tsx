import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { OutputOptions } from "../src/types";

afterEach(() => {
  cleanup();
  delete window.__TAURI__;
});

it("chooses and saves a native output directory and can reconnect PostgreSQL", async () => {
  const initial: OutputOptions = {
    enabled: false, directory: "", directory_template: "work-{row}", filename_prefix: "",
    metadata: true, synopsis: true, credits: true, ratings: true, episodes: true,
    raw: false, emby_nfo: false, poster: true, backdrop: false, stills: false, portraits: [],
  };
  const choose = vi.fn().mockRejectedValueOnce("目录选择器打开失败").mockResolvedValueOnce(null).mockResolvedValue("/Volumes/Export");
  const selection = { folder: "/Volumes/Library", paths: ["Library/Movie.mkv"],
    preview: { works: [{ kind: "movie", query: "Movie", file_count: 1 }], files: 1, skipped: 0, unrecognized: [] } };
  const establish = vi.fn().mockRejectedValueOnce("PostgreSQL 地址无效").mockResolvedValue(undefined);
  const invoke = vi.fn(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "output_settings": return { root: null, options: initial };
      case "save_output_settings": return { root: null, options: args?.options };
      case "pick_output_directory": return choose();
      case "choose_media_folder": return selection;
      case "create_folder_batch": return { id: 7, works: 1, files: 1, skipped: 0, unrecognized: [] };
      case "configure_database": return establish(args?.url);
      case "storage_configured": return true;
      case "check_storage": return;
      default: throw new Error(`Unexpected command ${command}`);
    }
  });
  window.__TAURI__ = { core: { invoke } };
  const { OutputSettings } = await import("../src/OutputSettings");
  const { Database } = await import("../src/Database");
  const { chooseMediaFolder, createFolderBatch, getStorageStatus } = await import("../src/api");
  expect(await chooseMediaFolder()).toEqual(selection);
  expect(invoke).not.toHaveBeenCalledWith("create_folder_batch", expect.anything());
  expect((await createFolderBatch(selection.paths)).id).toBe(7);
  expect(invoke).toHaveBeenCalledWith("create_folder_batch", { paths: selection.paths });
  const connected = vi.fn();
  const storage = vi.fn();
  render(<><OutputSettings /><Database storage="ready" onStorage={storage} onConnected={connected} /></>);

  const button = await screen.findByRole("button", { name: "选择目录" });
  const path = screen.getByLabelText("输出路径") as HTMLInputElement;
  expect(path.readOnly).toBe(true);
  fireEvent.click(button);
  expect(await screen.findByText("目录选择器打开失败")).toBeTruthy();
  fireEvent.click(button);
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  expect(path.value).toBe("");
  fireEvent.click(button);
  await waitFor(() => expect(path.value).toBe("/Volumes/Export"));
  fireEvent.click(screen.getByLabelText("写出刮削文件"));
  fireEvent.change(screen.getByLabelText("作品目录名"), { target: { value: "{title}" } });
  fireEvent.click(screen.getByLabelText("海报或封面"));
  fireEvent.click(screen.getByRole("button", { name: "保存输出设置" }));
  expect(await screen.findByText("已保存，设置应用于新建的文件夹批次。")).toBeTruthy();
  expect(invoke).toHaveBeenCalledWith("save_output_settings", {
    options: { ...initial, enabled: true, directory: "/Volumes/Export", directory_template: "{title}", poster: false },
  });

  const url = screen.getByLabelText("PostgreSQL 连接地址") as HTMLInputElement;
  fireEvent.change(url, { target: { value: "postgresql://localhost/test" } });
  const save = screen.getByRole("button", { name: "测试连接并保存" });
  fireEvent.click(save);
  expect(await screen.findByText("PostgreSQL 地址无效")).toBeTruthy();
  fireEvent.click(save);
  expect(await screen.findByText("已测试并保存 PostgreSQL 连接。")).toBeTruthy();
  expect(establish).toHaveBeenLastCalledWith("postgresql://localhost/test");
  expect(url.value).toBe("");
  expect(connected).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "测试当前数据库" }));
  await waitFor(() => expect(storage).toHaveBeenCalledWith("ready"));
  invoke.mockResolvedValueOnce(false);
  expect(await getStorageStatus()).toBe("none");
});
