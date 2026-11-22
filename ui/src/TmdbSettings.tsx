import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { getTmdbStatus, saveTmdb, testTmdb } from "./api";
import { ExternalLink } from "./Catalog";
import { errorText } from "./types";
import type { TmdbStatus } from "./types";

const labels: Record<TmdbStatus, string> = {
  unconfigured: "尚未配置 TMDB",
  environment: "当前使用环境变量中的凭据",
  saved: "已保存 TMDB 凭据",
};

export function TmdbSettings({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<TmdbStatus | null>(null);
  const [credential, setCredential] = useState("");
  const [action, setAction] = useState<"test" | "save" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const value = credential.trim();
  const hasSavedCredential = status === "environment" || status === "saved";

  useEffect(() => {
    let active = true;
    getTmdbStatus().then(
      (status) => { if (active) setStatus(status); },
      (error) => { if (active) setMessage({ text: errorText(error), error: true }); },
    );
    return () => { active = false; };
  }, []);

  async function test() {
    if (action || (!value && !hasSavedCredential)) return;
    setAction("test");
    setMessage(null);
    try {
      await testTmdb(value || null);
      setMessage({ text: "TMDB 连接测试成功。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (action || !value) return;
    setAction("save");
    setMessage(null);
    try {
      setStatus(await saveTmdb(value));
      setCredential("");
      setMessage({ text: "已验证并保存，电影和剧集采集即刻生效。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  return (
    <section aria-labelledby="tmdb-title">
      <div className="section-heading"><h2 id="tmdb-title">TMDB</h2></div>
      <form className="panel batch-form" onSubmit={save}>
        {status ? <p className="muted">{labels[status]}</p>
          : !message && <p className="muted" role="status">正在读取 TMDB 配置…</p>}
        <label htmlFor="tmdb-credential" className="batch-input">TMDB API 凭据</label>
        <input className="field w-full" id="tmdb-credential" type="password" autoComplete="off" spellCheck={false}
          placeholder="API Read Access Token 或 v3 API Key" maxLength={8192}
          value={credential} onChange={(event) => { setCredential(event.target.value); setMessage(null); }}
          disabled={action !== null} />
        <p className="muted">
          <ExternalLink value="https://www.themoviedb.org/settings/api" onError={onError}>获取 TMDB API 凭据 ↗</ExternalLink>
        </p>
        <div className="settings-actions">
          <button type="button" className="secondary" disabled={action !== null || (!value && !hasSavedCredential)}
            onClick={test}>{action === "test" ? "正在测试…" : "测试连接"}</button>
          <button type="submit" className="primary" disabled={action !== null || !value}>
            {action === "save" ? "正在验证并保存…" : "保存配置"}
          </button>
        </div>
        {message && <p className={message.error ? "batch-error" : "muted"}
          role={message.error ? "alert" : "status"}>{message.text}</p>}
      </form>
    </section>
  );
}
