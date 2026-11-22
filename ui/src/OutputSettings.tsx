import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { getOutputSettings, pickOutputDirectory, saveOutputSettings } from "./api";
import { errorText } from "./types";
import type { OutputOptions, OutputSettingsView, PortraitRole } from "./types";

const metadataFields = [
  ["synopsis", "简介与宣传语"],
  ["credits", "演职员名单"],
  ["ratings", "评分"],
  ["episodes", "季与分集"],
] as const;

const imageFields = [
  ["poster", "海报或封面"],
  ["backdrop", "背景图（Fanart）"],
  ["stills", "剧照与分集截图"],
] as const;

const mediaFields = [
  ["trailers", "预告片与先导预告"],
  ["theme_videos", "主题视频"],
  ["theme_music", "主题音乐"],
  ["extras", "花絮与其他片段"],
] as const;

const roles: Record<PortraitRole, string> = {
  actor: "演员",
  voice: "声优",
  director: "导演",
  writer: "编剧",
  crew: "其他幕后人员",
};

function exampleName(template: string) {
  const tokens: Record<string, string> = { title: "寄生虫", year: "2019", id: "TMDB-movie-496243", type: "movie", row: "7" };
  return template.trim().replace(/\{([a-z]+)\}/g, (placeholder, key: string) => tokens[key] ?? placeholder);
}

export function OutputSettings() {
  const [settings, setSettings] = useState<OutputSettingsView | null>(null);
  const [busy, setBusy] = useState<"pick" | "save" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);

  useEffect(() => {
    let active = true;
    getOutputSettings().then(
      (value) => { if (active) setSettings(value); },
      (error) => { if (active) setMessage({ text: errorText(error), error: true }); },
    );
    return () => { active = false; };
  }, []);

  function change<T extends keyof OutputOptions>(name: T, value: OutputOptions[T]) {
    setSettings((previous) => previous && { ...previous, options: { ...previous.options, [name]: value } });
    setMessage(null);
  }

  async function choose() {
    if (busy) return;
    setBusy("pick");
    try {
      const directory = await pickOutputDirectory();
      if (directory) change("directory", directory);
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setBusy(null);
    }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy || !settings) return;
    setBusy("save");
    try {
      setSettings(await saveOutputSettings(settings.options));
      setMessage({ text: "已保存，设置应用于新的刮削任务。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setBusy(null);
    }
  }

  const options = settings?.options;
  const mappings = options?.emby_paths ?? [];
  return (
    <section className="space-y-3" aria-labelledby="output-title">
      <div className="section-heading"><h2 id="output-title">刮削输出</h2></div>
      {settings && options ? (
        <form className="panel batch-form" onSubmit={save}>
          <fieldset className="output-naming" disabled={busy !== null}>
            <legend>Emby 媒体写回</legend>
            <p className="muted">NFO 和图片写到对应的媒体目录，写回时会更新同名文件。</p>
            {settings.root && (settings.emby_root
              ? <p className="muted">本地目录填写 <code>{settings.emby_root}</code> 下的相对路径，留空使用挂载根目录。</p>
              : <p className="muted">请先给 Web 服务配置可写媒体挂载 <code>EMBY_WRITEBACK_ROOT</code>。</p>)}
            {mappings.map((mapping, index) => (
              <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end" key={index}>
                <label className="batch-input">
                  Emby 端目录
                  <input className="field w-full" value={mapping.server} placeholder="/media/Movies"
                    onChange={(event) => change("emby_paths", mappings.map((row, i) =>
                      i === index ? { ...row, server: event.target.value } : row))} />
                </label>
                <label className="batch-input">
                  本地目录
                  <input className="field w-full" value={mapping.local} placeholder={settings.root ? "Movies" : "/Volumes/Media/Movies"}
                    onChange={(event) => change("emby_paths", mappings.map((row, i) =>
                      i === index ? { ...row, local: event.target.value } : row))} />
                </label>
                <button className="secondary" type="button" onClick={() => change("emby_paths", mappings.filter((_, i) => i !== index))}>移除</button>
              </div>
            ))}
            {(!settings.root || settings.emby_root) && <button className="secondary" type="button"
              disabled={mappings.length >= 32} onClick={() => change("emby_paths", [...mappings, { server: "", local: "" }])}>
              添加路径映射
            </button>}
          </fieldset>
          <label className="check-option"><input type="checkbox" checked={options.enabled}
            disabled={busy !== null} onChange={(event) => change("enabled", event.target.checked)} />将其他来源的刮削文件另存到输出目录</label>
          <label className="batch-input" htmlFor="output-directory">输出路径</label>
          <div className="grid w-full gap-2 sm:flex sm:[&>button]:shrink-0">
            <input className="field w-full" id="output-directory" value={options.directory} readOnly={settings.root === null}
              disabled={busy !== null} placeholder={settings.root === null ? "选择一个目录" : "留空写到挂载目录根部"}
              onChange={(event) => change("directory", event.target.value)} />
            {settings.root === null && <button className="secondary" type="button" disabled={busy !== null}
              onClick={choose}>选择目录</button>}
          </div>
          {settings.root && <p className="muted">容器根目录 <code>{settings.root}</code></p>}
          <fieldset className="output-naming" disabled={busy !== null}>
            <legend>输出命名</legend>
            <label className="batch-input" htmlFor="directory-name">作品目录名</label>
            <input className="field w-full" id="directory-name" value={options.directory_template} maxLength={160}
              placeholder="work-{row}" onChange={(event) => change("directory_template", event.target.value)} />
            <label className="batch-input" htmlFor="filename-prefix">资料文件名前缀</label>
            <input className="field w-full" id="filename-prefix" value={options.filename_prefix} maxLength={160}
              placeholder="例如 {title}" onChange={(event) => change("filename_prefix", event.target.value)} />
            <p className="muted">可用 <code>{"{title} {year} {id} {type} {row}"}</code>。Emby NFO 按视频名生成。</p>
            <output className="naming-preview">
              <span>目录示例</span>
              <code>{exampleName(options.directory_template) && options.directory_template.trim() !== "work-{row}"
                ? exampleName(options.directory_template) + "-7" : "work-7"}/{exampleName(options.filename_prefix) ? exampleName(options.filename_prefix) + "-" : ""}metadata.json</code>
            </output>
          </fieldset>
          <div className="output-groups grid w-full gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <fieldset disabled={busy !== null}>
              <legend>元数据</legend>
              <label className="check-option"><input type="checkbox" checked={options.metadata}
                onChange={(event) => change("metadata", event.target.checked)} />作品资料 JSON</label>
              {metadataFields.map(([key, label]) => <label className="check-option check-nested" key={key}>
                <input type="checkbox" checked={options[key]} disabled={!options.metadata && !options.emby_nfo}
                  onChange={(event) => change(key, event.target.checked)} />{label}
              </label>)}
              <label className="check-option"><input type="checkbox" checked={options.raw}
                onChange={(event) => change("raw", event.target.checked)} />原始来源 JSON</label>
              <label className="check-option"><input type="checkbox" checked={options.emby_nfo}
                onChange={(event) => change("emby_nfo", event.target.checked)} />Emby NFO</label>
            </fieldset>
            <fieldset disabled={busy !== null}>
              <legend>作品图片</legend>
              {imageFields.map(([key, label]) => <label className="check-option" key={key}>
                <input type="checkbox" checked={options[key]} onChange={(event) => change(key, event.target.checked)} />{label}
              </label>)}
            </fieldset>
            <fieldset disabled={busy !== null}>
              <legend>附加媒体</legend>
              {mediaFields.map(([key, label]) => <label className="check-option" key={key}>
                <input type="checkbox" checked={options[key]} onChange={(event) => change(key, event.target.checked)} />{label}
              </label>)}
              <p className="muted">可下载的媒体保存为文件。只有播放页时，保存链接。</p>
            </fieldset>
            <fieldset disabled={busy !== null}>
              <legend>人物头像</legend>
              {(Object.keys(roles) as PortraitRole[]).map((role) => (
                <label className="check-option" key={role}>
                  <input type="checkbox" checked={options.portraits.includes(role)}
                    onChange={(event) => change("portraits", event.target.checked
                      ? [...options.portraits, role] : options.portraits.filter((value) => value !== role))} />{roles[role]}
                </label>
              ))}
            </fieldset>
          </div>
          <button className="primary" type="submit" disabled={busy !== null}>
            {busy === "save" ? "正在保存…" : "保存输出设置"}
          </button>
        </form>
      ) : !message && <p className="muted" role="status">正在读取输出设置…</p>}
      {message && <p className={message.error ? "batch-error mt-2" : "muted mt-2"}
        role={message.error ? "alert" : "status"}>{message.text}</p>}
    </section>
  );
}
