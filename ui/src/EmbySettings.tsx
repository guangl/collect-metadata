import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { getEmbySettings, isDesktop, openUrl, saveEmby, testEmby } from "./api";
import { errorText } from "./types";
import type { EmbySettingsView } from "./types";

function organizerPage(server: string | undefined): string | null {
  if (!server) return null;
  try {
    const url = new URL(server);
    if (!/^https?:$/.test(url.protocol) || url.username || url.password) return null;
    url.pathname = url.pathname.replace(/\/+$/, "") + "/web/index.html";
    url.search = "";
    url.hash = "!/configurationpage?name=collectmetadata";
    return url.href;
  } catch { return null; }
}

export function EmbySettings() {
  const [settings, setSettings] = useState<EmbySettingsView | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [action, setAction] = useState<"test" | "save" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);

  useEffect(() => {
    let active = true;
    getEmbySettings().then(
      (value) => { if (active) setSettings(value); },
      (error) => { if (active) setMessage({ text: errorText(error), error: true }); },
    );
    return () => { active = false; };
  }, []);

  async function run(next: "test" | "save") {
    if (!settings || action) return;
    setAction(next);
    setMessage(null);
    try {
      const input = { enabled: settings.enabled, server: settings.server, api_key: apiKey };
      if (next === "test") {
        await testEmby(input);
        setMessage({ text: "Emby 媒体库读取成功。" });
      } else {
        setSettings(await saveEmby(input));
        setApiKey("");
        setMessage({ text: "已保存 Emby 设置。" });
      }
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void run("save");
  }

  const organizer = organizerPage(settings?.server);
  return (
    <section aria-labelledby="emby-title">
      <div className="section-heading"><h2 id="emby-title">家庭 Emby</h2></div>
      {settings ? <form className="panel batch-form" onSubmit={submit}>
        <label className="check-option"><input type="checkbox" checked={settings.enabled} disabled={action !== null}
          onChange={(event) => setSettings({ ...settings, enabled: event.target.checked })} />优先读取 Emby 媒体库</label>
        <p className="muted">此选项用于文件夹刮削。保存连接后，工作台会自动浏览全部 Emby 媒体库。</p>
        <label htmlFor="emby-server" className="batch-input">Emby 服务地址</label>
        <input className="field w-full" id="emby-server" type="url" spellCheck={false} disabled={action !== null}
          placeholder="http://服务器:8096" value={settings.server}
          onChange={(event) => setSettings({ ...settings, server: event.target.value })} />
        <label htmlFor="emby-key" className="batch-input">Emby API 密钥</label>
        <input className="field w-full" id="emby-key" type="password" autoComplete="off" spellCheck={false} disabled={action !== null}
          placeholder={settings.configured ? "已保存，留空沿用" : "填写 Emby API 密钥"} value={apiKey}
          onChange={(event) => setApiKey(event.target.value)} />
        <div className="settings-actions">
          <button type="button" className="secondary" disabled={action !== null || !settings.server.trim()}
            onClick={() => { void run("test"); }}>{action === "test" ? "正在测试…" : "测试 Emby 连接"}</button>
          <button type="submit" className="primary" disabled={action !== null}>
            {action === "save" ? "正在保存…" : "保存 Emby 设置"}
          </button>
        </div>
      </form> : !message && <p className="muted" role="status">正在读取 Emby 设置…</p>}
      {message && <p className={message.error ? "batch-error mt-2" : "muted mt-2"}
        role={message.error ? "alert" : "status"}>{message.text}</p>}
      <div className="panel mt-4 space-y-2">
        <h3>自动整理与重命名</h3>
        <p className="muted">安装新版插件后，在 Emby 管理后台选择允许整理的媒体库，并设置目录和文件名模板。整理任务在 Emby 服务端运行。</p>
        {organizer && <a className="inline-block text-sm" href={organizer} target="_blank" rel="noopener noreferrer" onClick={isDesktop ? (event) => {
          event.preventDefault();
          void openUrl(organizer).catch(() => setMessage({ text: "无法打开 Emby 设置，请在浏览器登录 Emby 管理后台。", error: true }));
        } : undefined}>打开 Emby 自动整理设置</a>}
      </div>
    </section>
  );
}
