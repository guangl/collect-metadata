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
  ["backdrop", "背景图"],
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
      setMessage({ text: "已保存，设置应用于新建的文件夹批次。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setBusy(null);
    }
  }

  const options = settings?.options;
  return (
    <section className="settings-output" aria-labelledby="output-title">
      <div className="section-heading"><h2 id="output-title">刮削输出</h2></div>
      {settings && options ? (
        <form className="batch-form settings-form output-form" onSubmit={save}>
          <label className="check-option"><input type="checkbox" checked={options.enabled}
            disabled={busy !== null} onChange={(event) => change("enabled", event.target.checked)} />写出刮削文件</label>
          <label className="batch-input" htmlFor="output-directory">输出路径</label>
          <div className="output-directory">
            <input id="output-directory" value={options.directory} readOnly={settings.root === null}
              disabled={busy !== null} placeholder={settings.root === null ? "选择一个目录" : "留空写到挂载目录根部"}
              onChange={(event) => change("directory", event.target.value)} />
            {settings.root === null && <button className="secondary" type="button" disabled={busy !== null}
              onClick={choose}>选择目录</button>}
          </div>
          {settings.root && <p className="muted">容器根目录 <code>{settings.root}</code></p>}
          <fieldset className="output-naming" disabled={busy !== null}>
            <legend>输出命名</legend>
            <label className="batch-input" htmlFor="directory-name">作品目录名</label>
            <input id="directory-name" value={options.directory_template} maxLength={160}
              placeholder="work-{row}" onChange={(event) => change("directory_template", event.target.value)} />
            <label className="batch-input" htmlFor="filename-prefix">资料文件名前缀</label>
            <input id="filename-prefix" value={options.filename_prefix} maxLength={160}
              placeholder="例如 {title}" onChange={(event) => change("filename_prefix", event.target.value)} />
            <p className="muted">可用 <code>{"{title} {year} {id} {type} {row}"}</code>。Emby NFO 按视频名生成。</p>
            <output className="naming-preview">
              <span>目录示例</span>
              <code>{exampleName(options.directory_template) && options.directory_template.trim() !== "work-{row}"
                ? exampleName(options.directory_template) + "-7" : "work-7"}/{exampleName(options.filename_prefix) ? exampleName(options.filename_prefix) + "-" : ""}metadata.json</code>
            </output>
          </fieldset>
          <div className="output-groups">
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
      {message && <p className={message.error ? "batch-error output-feedback" : "muted output-feedback"}
        role={message.error ? "alert" : "status"}>{message.text}</p>}
    </section>
  );
}
